#!/usr/bin/env python3
# Minimal SquashFS 4.0 extractor for Atheros "lzma adaptive" (5-byte header) blocks.
import struct, lzma, os, sys, stat

IMG = sys.argv[1] if len(sys.argv)>1 else '/Volumes/MacEXT/code/logitechharmonyhub/backups/mtd3.bin'
OUT = sys.argv[2] if len(sys.argv)>2 else '/tmp/harmony_rootfs'
d = open(IMG,'rb').read()

def decomp(blk):
    # custom: props(1) + dict_size(4 LE) + raw lzma1 stream
    props = blk[0]
    dsize = struct.unpack_from('<I', blk, 1)[0]
    stream = blk[5:]
    pb = props//45; lp=(props//9)%5; lc=props%9
    filt=[{"id":lzma.FILTER_LZMA1,"lc":lc,"lp":lp,"pb":pb,"dict_size":dsize if dsize else 1<<17}]
    return lzma.decompress(stream, format=lzma.FORMAT_RAW, filters=filt)

# superblock
sb = struct.unpack_from('<IIIIIHHHHHHQQQQQQQQ', d, 0)
(magic,inodes,mkfs,block_size,fragments,comp,block_log,flags,no_ids,
 s_maj,s_min,root_inode,bytes_used,id_table,xattr_table,inode_table,
 dir_table,frag_table,export_table) = sb
assert magic==0x73717368

# ---- read metadata starting at an absolute offset, return (bytes, list of block start offsets) ----
def read_metadata(start, end):
    out=bytearray(); offsets=[]
    off=start
    while off < end:
        offsets.append((off, len(out)))
        hdr=struct.unpack_from('<H', d, off)[0]
        comp = (hdr & 0x8000)==0
        size = hdr & 0x7fff
        blk = d[off+2:off+2+size]
        out += decomp(blk) if comp else blk
        off += 2+size
    return bytes(out), offsets

inode_blob, inode_offs = read_metadata(inode_table, dir_table)
dir_blob, dir_offs = read_metadata(dir_table, frag_table)

# map: absolute disk offset of metadata block start -> offset within decompressed blob
def blockmap(offsets):
    return {a:b for a,b in offsets}
inode_map = blockmap(inode_offs)
dir_map = blockmap(dir_offs)

def meta_pos(table_start, ref, themap):
    # ref = (block_start_offset<<16) | offset_in_block ; block_start is relative to table_start
    block = ref >> 16
    inblk = ref & 0xffff
    abs_block = table_start + block
    base = themap[abs_block]
    return base + inblk

# ---- fragment table ----
frag_entries = fragments
# frag index table: array of u64 pointers to metadata blocks containing fragment entries
n_findex = (frag_entries*16 + 8191)//8192
findex = struct.unpack_from('<%dQ'%n_findex, d, frag_table)
frag_meta=bytearray()
for p in findex:
    hdr=struct.unpack_from('<H',d,p)[0]; size=hdr&0x7fff; c=(hdr&0x8000)==0
    blk=d[p+2:p+2+size]; frag_meta += decomp(blk) if c else blk
def frag_entry(i):
    start, size = struct.unpack_from('<QI', frag_meta, i*16)
    return start, size

INODE_DIR=1; INODE_FILE=2; INODE_SYM=3; INODE_BLK=4; INODE_CHR=5; INODE_FIFO=6; INODE_SOCK=7
INODE_LDIR=8; INODE_LFILE=9; INODE_LSYM=10; INODE_LBLK=11; INODE_LCHR=12; INODE_LFIFO=13; INODE_LSOCK=14

def read_inode(pos):
    typ, mode, uid, gid, mtime, inode_number = struct.unpack_from('<HHHHII', inode_blob, pos)
    base = dict(type=typ,mode=mode,uid=uid,gid=gid,mtime=mtime,num=inode_number)
    p = pos+16
    if typ==INODE_DIR:
        start_block, nlink, file_size, offset, parent = struct.unpack_from('<IIHHI', inode_blob, p)
        base.update(dir_start=start_block, dir_offset=offset, file_size=file_size)
    elif typ==INODE_LDIR:
        nlink, file_size, start_block, parent, icount, offset, xattr = struct.unpack_from('<IIIIHHI', inode_blob, p)
        base.update(dir_start=start_block, dir_offset=offset, file_size=file_size)
    elif typ==INODE_FILE:
        start_block, frag, frag_off, file_size = struct.unpack_from('<IIII', inode_blob, p)
        nblocks = (file_size// block_size) if frag!=0xffffffff else ((file_size+block_size-1)//block_size)
        sizes = struct.unpack_from('<%dI'%nblocks, inode_blob, p+16)
        base.update(start_block=start_block, frag=frag, frag_off=frag_off, file_size=file_size, sizes=sizes)
    elif typ==INODE_LFILE:
        start_block, file_size, sparse, nlink, frag, frag_off, xattr = struct.unpack_from('<QQQIIII', inode_blob, p)
        nblocks = (file_size// block_size) if frag!=0xffffffff else ((file_size+block_size-1)//block_size)
        sizes = struct.unpack_from('<%dI'%nblocks, inode_blob, p+44)
        base.update(start_block=start_block, frag=frag, frag_off=frag_off, file_size=file_size, sizes=sizes)
    elif typ in (INODE_SYM,INODE_LSYM):
        nlink, tgt_size = struct.unpack_from('<II', inode_blob, p)
        target = inode_blob[p+8:p+8+tgt_size].decode('latin1')
        base.update(symlink=target)
    elif typ in (INODE_CHR,INODE_BLK,INODE_LCHR,INODE_LBLK):
        nlink, dev = struct.unpack_from('<II', inode_blob, p)
        base.update(dev=dev)
    elif typ in (INODE_FIFO,INODE_SOCK,INODE_LFIFO,INODE_LSOCK):
        pass
    return base

def list_dir(inode):
    pos = meta_pos(dir_table, (inode['dir_start']<<16)|inode['dir_offset'], dir_map)
    size = inode['file_size']-3
    end = pos+size
    entries=[]
    while pos < end:
        count, start_block, inode_base = struct.unpack_from('<III', dir_blob, pos)
        pos += 12
        for _ in range(count+1):
            offset, inode_off, etype, nsize = struct.unpack_from('<hhHH', dir_blob, pos)
            pos += 8
            name = dir_blob[pos:pos+nsize+1].decode('latin1')
            pos += nsize+1
            entries.append((name, (start_block<<16)|offset))
    return entries

def file_data(inode):
    out=bytearray()
    off = inode['start_block']
    for sz in inode.get('sizes',()):
        comp = (sz & 0x1000000)==0
        realsz = sz & 0xffffff
        if realsz==0:
            out += b'\x00'*block_size
            continue
        blk = d[off:off+realsz]
        out += decomp(blk) if comp else blk
        off += realsz
    if inode['frag']!=0xffffffff:
        fstart, fsize = frag_entry(inode['frag'])
        comp=(fsize & 0x1000000)==0; rs=fsize & 0xffffff
        fblk=d[fstart:fstart+rs]
        fdata = decomp(fblk) if comp else fblk
        out += fdata[inode['frag_off']:inode['frag_off']+(inode['file_size']-len(out))]
    return bytes(out[:inode['file_size']])

manifest=[]
def walk(inode_ref, path):
    pos = meta_pos(inode_table, inode_ref, inode_map)
    inode = read_inode(pos)
    typ=inode['type']
    full = os.path.join(OUT, path) if path else OUT
    if typ in (INODE_DIR,INODE_LDIR):
        os.makedirs(full, exist_ok=True)
        for name, child_ref in list_dir(inode):
            walk(child_ref, os.path.join(path,name) if path else name)
    elif typ in (INODE_FILE,INODE_LFILE):
        data=file_data(inode)
        with open(full,'wb') as f: f.write(data)
        try: os.chmod(full, inode['mode']&0o7777)
        except: pass
        manifest.append((path,'file',inode['mode'],len(data)))
    elif typ in (INODE_SYM,INODE_LSYM):
        try:
            if os.path.lexists(full): os.remove(full)
            os.symlink(inode['symlink'], full)
        except Exception as e: pass
        manifest.append((path,'symlink->'+inode['symlink'],inode['mode'],0))
    elif typ in (INODE_CHR,INODE_LCHR):
        manifest.append((path,'chardev %d,%d'%((inode['dev']>>8)&0xff if inode['dev']<0x10000 else (inode['dev']>>8), inode['dev']&0xff), inode['mode'],0))
    elif typ in (INODE_BLK,INODE_LBLK):
        manifest.append((path,'blockdev',inode['mode'],0))
    else:
        manifest.append((path,'special t%d'%typ,inode['mode'],0))

walk(root_inode, '')
# write manifest
with open(os.path.join(OUT,'..','harmony_manifest.txt'),'w') as f:
    for p,t,m,s in sorted(manifest):
        f.write('%-50s %-22s %06o %d\n'%(p,t,m,s))
print('extracted', len(manifest),'entries to',OUT)

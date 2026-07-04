#!/usr/bin/env python3
"""mipsdis.py — minimal big-endian MIPS32 RE helper (capstone) for the stripped Harmony `hal`
and the (symbolized) cc2544.ko. Hand-rolled ELF32-BE parsing, no pyelftools dependency.

Usage:
  mipsdis.py <elf> str  <substring>          # find a string + its vaddr(s)
  mipsdis.py <elf> xref <0xADDR>             # find lui/addiu (or lw-from-.data) sites that build ADDR
  mipsdis.py <elf> dis  <0xADDR> [count]     # disassemble count insns (default: until 'jr $ra')
  mipsdis.py <elf> sym  <substr>             # list matching symbols (name -> vaddr) [.ko has symtab]
  mipsdis.py <elf> at   <0xADDR>             # nearest preceding symbol to ADDR
"""
import sys, struct

class ELF:
    def __init__(self, path):
        self.d = open(path, "rb").read()
        d = self.d
        assert d[:4] == b"\x7fELF", "not ELF"
        assert d[5] == 2, "expected big-endian (EI_DATA=2)"
        (self.e_type, self.e_machine, _v, self.e_entry, self.e_phoff, self.e_shoff,
         _flags, _ehsz, self.e_phentsize, self.e_phnum, self.e_shentsize, self.e_shnum,
         self.e_shstrndx) = struct.unpack_from(">HHIIIIIHHHHHH", d, 16)
        self.sections = []   # (name, sh_type, addr, offset, size, link, entsize)
        shs = []
        for i in range(self.e_shnum):
            off = self.e_shoff + i*self.e_shentsize
            name, typ, flags, addr, offset, size, link, info, align, entsz = \
                struct.unpack_from(">IIIIIIIIII", d, off)
            shs.append((name, typ, addr, offset, size, link, entsz))
        strtab_off = shs[self.e_shstrndx][3]
        def nm(o):
            e = d.index(b"\x00", strtab_off+o); return d[strtab_off+o:e].decode("latin1")
        self.sections = [(nm(s[0]), s[1], s[2], s[3], s[4], s[5], s[6]) for s in shs]
        # symbols (.symtab preferred, else .dynsym)
        self.syms = []  # (name, value, size, shndx)
        for want in (".symtab", ".dynsym"):
            sec = self.sec(want)
            if not sec: continue
            _n, _t, _a, off, size, link, ent = sec
            stroff = self.sections[link][3]
            for o in range(off, off+size, ent or 16):
                st_name, st_val, st_size, st_info, st_other, st_shndx = \
                    struct.unpack_from(">IIIBBH", d, o)
                if st_name == 0: continue
                e = d.index(b"\x00", stroff+st_name)
                self.syms.append((d[stroff+st_name:e].decode("latin1"), st_val, st_size, st_shndx))
            if self.syms: break

        # full ordered .dynsym (for MIPS GOT->symbol mapping) + .dynamic tags
        self.dynsyms = []
        sec = self.sec(".dynsym")
        if sec:
            _n, _t, _a, off, size, link, ent = sec
            stroff = self.sections[link][3]
            for o in range(off, off+size, ent or 16):
                st_name, st_val = struct.unpack_from(">II", self.d, o)
                nm = ""
                if st_name:
                    e = self.d.index(b"\x00", stroff+st_name); nm = self.d[stroff+st_name:e].decode("latin1")
                self.dynsyms.append((nm, st_val))
        self.dyn = {}
        dsec = self.sec(".dynamic")
        if dsec:
            off, size = dsec[3], dsec[4]
            for o in range(off, off+size, 8):
                tag, val = struct.unpack_from(">II", self.d, o)
                self.dyn[tag] = val
                if tag == 0: break
        # MIPS GOT: gp = pltgot + 0x7ff0; global GOT[i>=local_gotno] -> dynsym[gotsym + (i-local_gotno)]
        self.pltgot = self.dyn.get(3)                    # DT_PLTGOT
        self.local_gotno = self.dyn.get(0x7000000a, 0)   # DT_MIPS_LOCAL_GOTNO
        self.gotsym = self.dyn.get(0x70000013, 0)        # DT_MIPS_GOTSYM
        self.gp = (self.pltgot + 0x7ff0) if self.pltgot else None

    def got_name(self, gp_off):
        """Resolve `lw $t9, gp_off($gp)` -> dynamic symbol name, or ''."""
        if self.gp is None: return ""
        addr = (self.gp + gp_off) & 0xffffffff
        idx = (addr - self.pltgot) // 4
        if idx < self.local_gotno: return ""
        sidx = self.gotsym + (idx - self.local_gotno)
        if 0 <= sidx < len(self.dynsyms): return self.dynsyms[sidx][0]
        return ""

    def sec(self, name):
        for s in self.sections:
            if s[0] == name: return s
        return None

    def v2o(self, vaddr):
        for _n, _t, addr, off, size, _l, _e in self.sections:
            if addr and addr <= vaddr < addr+size:
                return off + (vaddr-addr)
        return None

    def read(self, vaddr, n):
        o = self.v2o(vaddr)
        return self.d[o:o+n] if o is not None else b""

    def find_str(self, sub):
        b = sub.encode() if isinstance(sub, str) else sub
        out = []
        for _n, typ, addr, off, size, _l, _e in self.sections:
            if not addr: continue
            blob = self.d[off:off+size]
            i = blob.find(b, 0)
            while i != -1:
                out.append(addr+i); i = blob.find(b, i+1)
        return out

    def nearest_sym(self, vaddr):
        best = None
        for name, val, size, shndx in self.syms:
            if val <= vaddr and (best is None or val > best[1]):
                best = (name, val)
        return best

def cs_mips():
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_MIPS, capstone.CS_MODE_MIPS32 | capstone.CS_MODE_BIG_ENDIAN)
    md.detail = False
    return md

def cmd_dis(elf, vaddr, count=None):
    md = cs_mips()
    data = elf.read(vaddr, (count or 4000)*4)
    n = 0
    import re as _re
    for ins in md.disasm(data, vaddr):
        tag = ""
        # resolve `lw $t9, off($gp)` -> GOT function name (the next jalr $t9 calls it)
        if ins.mnemonic == "lw" and "$t9," in ins.op_str and "($gp)" in ins.op_str:
            m = _re.search(r'(-?0x[0-9a-f]+)\(\$gp\)', ins.op_str)
            if m:
                off = int(m.group(1), 16)
                nm = elf.got_name(off)
                if nm: tag = f"   ; -> {nm}()"
        if not tag:
            s = elf.nearest_sym(ins.address)
            tag = f"  <{s[0]}+{ins.address-s[1]:#x}>" if s else ""
        print(f"{ins.address:#010x}: {ins.mnemonic:8} {ins.op_str}{tag}")
        n += 1
        if count is None and ins.mnemonic == "jr" and "$ra" in ins.op_str:
            # also print the delay slot then stop
            continue_addr = ins.address + 4
            for d in md.disasm(elf.read(continue_addr, 4), continue_addr):
                print(f"{d.address:#010x}: {d.mnemonic:8} {d.op_str}")
            break
        if count and n >= count: break

def cmd_xref(elf, target):
    md = cs_mips()
    hi16 = (target >> 16) & 0xffff
    lo16 = target & 0xffff
    # account for sign-extension of the addiu/lw immediate
    hi_alt = (hi16 + 1) & 0xffff
    found = []
    for _n, typ, addr, off, size, _l, _e in elf.sections:
        if not addr or typ != 1:  # PROGBITS
            continue
        data = elf.d[off:off+size]
        # track last lui per register
        luis = {}
        for ins in md.disasm(data, addr):
            m, ops = ins.mnemonic, ins.op_str
            if m == "lui":
                try:
                    reg, imm = ops.split(", "); luis[reg] = (int(imm, 16), ins.address)
                except Exception: pass
            elif m in ("addiu", "ori", "lw", "addi"):
                parts = ops.replace("(", ", ").replace(")", "").split(", ")
                if len(parts) >= 2:
                    base = parts[-1]
                    if base in luis:
                        hival = luis[base][0]
                        try: imm = int(parts[1], 16) if parts[1].startswith("0x") or parts[1].lstrip("-").isdigit() else None
                        except Exception: imm = None
                        if imm is not None:
                            if imm & 0x8000: imm -= 0x10000
                            eff = ((hival << 16) + imm) & 0xffffffff
                            if eff == target:
                                found.append((luis[base][1], ins.address, m))
    for lui_at, use_at, m in found:
        print(f"xref {target:#x}: lui@{lui_at:#010x} + {m}@{use_at:#010x}")
    if not found:
        print(f"(no lui/addiu pair builds {target:#x}; may be loaded from GOT/.data pointer)")

def main():
    if len(sys.argv) < 3: print(__doc__); sys.exit(2)
    elf = ELF(sys.argv[1]); cmd = sys.argv[2]
    if cmd == "str":
        for v in elf.find_str(sys.argv[3]): print(f"{v:#010x}  {sys.argv[3]!r}")
    elif cmd == "xref":
        cmd_xref(elf, int(sys.argv[3], 16))
    elif cmd == "ptr":
        target = int(sys.argv[3], 16); needle = struct.pack(">I", target)
        for _n, typ, addr, off, size, _l, _e in elf.sections:
            if not addr: continue
            blob = elf.d[off:off+size]; i = blob.find(needle)
            while i != -1:
                loc = addr+i
                # show the 8 bytes after (likely the paired handler pointer in a {name,fn} table)
                after = struct.unpack_from(">I", elf.d, off+i+4)[0] if i+8 <= len(blob) else 0
                before = struct.unpack_from(">I", elf.d, off+i-4)[0] if i >= 4 else 0
                print(f"ptr {target:#x} @ {loc:#010x} ({_n})  prev={before:#010x} next={after:#010x}")
                i = blob.find(needle, i+1)
    elif cmd == "dis":
        cnt = int(sys.argv[4]) if len(sys.argv) > 4 else None
        cmd_dis(elf, int(sys.argv[3], 16), cnt)
    elif cmd == "sym":
        for name, val, size, sh in elf.syms:
            if sys.argv[3].lower() in name.lower(): print(f"{val:#010x}  {name}")
    elif cmd == "at":
        s = elf.nearest_sym(int(sys.argv[3], 16)); print(s)
    else: print(__doc__)

if __name__ == "__main__":
    main()

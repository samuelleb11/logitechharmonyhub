#!/bin/zsh
# Dump every MTD partition over the console (RLE+base64 via /tmp/r.lua),
# decode on the host, and verify each against the device's reference md5.
cd /Volumes/MacEXT/code/logitechharmonyhub

typeset -A ref to
ref[0]=eb99f5aaaced4ad73ddecfc7d8650374   # u-boot
ref[1]=6b289896490a4e2bb83668a2bb8352a4   # kernel
ref[2]=e97769247863eb975af9b8f35ef24c94   # kernel2
ref[3]=6becf6b6afcace8d19d91d9e665e1596   # root
ref[4]=b752c133cf0ceb62bf912629b756dcd3   # data
ref[5]=6296e1340e6a2f66a0361e28977a519b   # cache
ref[6]=30fa6551a583b80f5bed8a6ea3f7f144   # mfg
to[0]=120; to[1]=300; to[2]=300; to[3]=700; to[4]=300; to[5]=120; to[6]=60

mkdir -p backups
fail=0
for n in 0 1 2 3 4 5 6; do
  dev=/dev/mtd$n; out=backups/mtd${n}.bin
  echo "[$(date +%H:%M:%S)] dumping mtd$n (timeout ${to[$n]}s) ..."
  python3 uartctl.py send "lua /tmp/r.lua $dev"
  if ! python3 uartctl.py wait "===RLE64END $dev rawbytes=[0-9]+===" ${to[$n]} >/dev/null 2>&1; then
    echo "[$(date +%H:%M:%S)] mtd$n WAIT TIMEOUT"; fail=1; continue
  fi
  python3 tools/extract_rle64.py /tmp/uart-latest.log $dev $out >/tmp/extract_mtd$n.txt 2>&1
  got=$(md5 -q $out 2>/dev/null)
  wire=$(grep wire /tmp/extract_mtd$n.txt)
  if [ "$got" = "${ref[$n]}" ]; then
    echo "[$(date +%H:%M:%S)] mtd$n VERIFIED md5=$got | $wire"
  else
    echo "[$(date +%H:%M:%S)] mtd$n MD5 MISMATCH got=$got ref=${ref[$n]}"; cat /tmp/extract_mtd$n.txt; fail=1
  fi
done

echo "----"
if [ $fail -eq 0 ]; then
  # assemble full 16MB image in partition order
  cat backups/mtd0.bin backups/mtd1.bin backups/mtd2.bin backups/mtd3.bin \
      backups/mtd4.bin backups/mtd5.bin backups/mtd6.bin > backups/harmony-fullflash-16MB.bin
  echo "[$(date +%H:%M:%S)] assembled $(ls -l backups/harmony-fullflash-16MB.bin | awk '{print $5}') bytes -> backups/harmony-fullflash-16MB.bin"
  echo "full-image md5: $(md5 -q backups/harmony-fullflash-16MB.bin)"
fi
echo "[$(date +%H:%M:%S)] ALL DONE (fail=$fail)"

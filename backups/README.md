# Harmony Hub (Pimento) — full firmware backup

Captured **2026-06-21** over the FTDI UART console (115200 8N1) from the live
device booted to `init=/bin/sh`. Each partition was read with `lua` on-device,
RLE-compressed (0xFF runs) + base64 over the console, decoded on the host, and
verified against the device's own `md5sum`. The assembled image was additionally
cross-checked against a whole-chip `cat /dev/mtd0..6 | md5sum` on the device.

See `../tools/dumpflash_rle.lua` (encoder) and `../tools/extract_rle64.py` (host decoder).

## Files & verified md5 (all confirmed == device)

| file | partition | offset | size | md5 |
|------|-----------|--------|------|-----|
| `mtd0.bin` | u-boot  | `0x000000` | 64 KB   | `eb99f5aaaced4ad73ddecfc7d8650374` |
| `mtd1.bin` | kernel  | `0x010000` | 960 KB  | `6b289896490a4e2bb83668a2bb8352a4` |
| `mtd2.bin` | kernel2 | `0x100000` | 960 KB  | `e97769247863eb975af9b8f35ef24c94` |
| `mtd3.bin` | root    | `0x1f0000` | 4096 KB | `6becf6b6afcace8d19d91d9e665e1596` |
| `mtd4.bin` | data    | `0x5f0000` | 5120 KB | `b752c133cf0ceb62bf912629b756dcd3` |
| `mtd5.bin` | cache   | `0xaf0000` | 5120 KB | `6296e1340e6a2f66a0361e28977a519b` |
| `mtd6.bin` | mfg     | `0xff0000` | 64 KB   | `30fa6551a583b80f5bed8a6ea3f7f144` |
| `harmony-fullflash-16MB.bin` | **whole chip** | `0x000000` | 16 MB | `48154ae8782176ed54d79503cb22ea53` |

`harmony-fullflash-16MB.bin` = mtd0..mtd6 concatenated in order (contiguous, no gaps).

## Notes
- **`mtd6` (mfg) is per-unit and irreplaceable** — radio calibration + MAC/identity. Keep it safe.
- `mtd4` (data) and `mtd5` (cache) are volatile jffs2 (user data / runtime cache); they reflect device state at capture time.
- This is a read-only backup. To **restore**, flash partitions individually from U-Boot or Linux MTD — do NOT blindly write the 16 MB blob without confirming the write tool, erase semantics, and especially that `mfg` is preserved. The bootloader override used for capture (`console=ttyS0 init=/bin/sh`) was RAM-only and left the stored env untouched.

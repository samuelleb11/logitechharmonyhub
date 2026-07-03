# Firmware Backup

A complete, byte-exact backup of the 16 MB NOR flash was captured **2026-06-21**
over the FTDI UART console and verified against the device. Artifacts + per-partition
md5s live in [`../backups/`](../backups/) (see [`../backups/README.md`](../backups/README.md)).

## Why it was non-trivial
This BusyBox build has **none** of the usual tools: no `dd`, `od`, `xxd`, `hexdump`,
`base64`, `uuencode`, `nc`, `tftp`, `wget`, `stty`, `mtd_debug`, `nanddump`,
`flashcp`, `gzip`, `python`, or `perl`. No Ethernet carrier in Linux (only `lo`), and
USB is gadget-mode only. So the usual "dump to network" and "raw binary over serial"
(needs `stty raw`) paths are both unavailable.

**But `/usr/bin/lua` (5.1) is present and byte-safe**, and `md5sum` works on
`/dev/mtdN`. That is the whole backup channel.

## Method
`lua` on the device reads each `/dev/mtdN`, **RLE-compresses the erased `0xFF`
runs**, base64-encodes, and streams it over the console. The host captures the
console log, decodes, and verifies each partition's md5 against the device's own
`md5sum`. Tools (in [`../tools/`](../tools/)):

- [`dumpflash_rle.lua`](../tools/dumpflash_rle.lua) — on-device encoder (uploaded to `/tmp/r.lua`). Frames output as `===RLE64BEGIN <path>===` … `===RLE64END <path> rawbytes=N===`.
- [`extract_rle64.py`](../tools/extract_rle64.py) — host decoder + verifier.
- [`dumpflash.lua`](../tools/dumpflash.lua) / [`extract_b64.py`](../tools/extract_b64.py) — plain base64 (no RLE) variant.
- [`backup_all.sh`](../tools/backup_all.sh) — dumps all 7 partitions, verifies each, assembles the full image.

Upload trick (no quoting pain): `uartctl.py send 'cat > /tmp/r.lua'`, then
`uartctl.py send "$(cat tools/dumpflash_rle.lua)"`, then `uartctl.py key ctrl-d`.

## Why RLE
NOR flash is **~60 % erased `0xFF`** (cache 99.98 %, data 80 %, mfg 94 %), so
compressing `0xFF` runs cut the wire from ~21 MB to ~9 MB — **~14 min instead of
~33** at 115200. Kernels/rootfs are incompressible (wire ≈ 130 % of raw, same as
plain base64). At 115200 the UART is the bottleneck, not lua (~1 MB/s), so per-byte
lua encoding keeps up fine.

## Verification
Every partition's decoded md5 matched the device's `md5sum`. The assembled 16 MB
image was additionally cross-checked against a whole-chip `cat /dev/mtd0..6 | md5sum`
on the device:

```
device : 48154ae8782176ed54d79503cb22ea53
host   : 48154ae8782176ed54d79503cb22ea53   (backups/harmony-fullflash-16MB.bin)
```

So the backup is proven identical to the live flash, not merely captured.

> **mtd6 (mfg) is per-unit and irreplaceable** — radio calibration + MAC/identity.
> Keep it safe; never erase it.

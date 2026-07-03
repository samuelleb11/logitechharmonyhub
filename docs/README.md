# Logitech Harmony Hub (Pimento) — Reverse Engineering Notes

Working notes and gathered specs for the Logitech Harmony Hub, accessed over its
TTL UART console.

| Doc | Contents |
|-----|----------|
| [board-access-runbook.md](board-access-runbook.md) | **⭐ The end-to-end playbook:** powered-off board → untethered root shell over Wi-Fi (UART, U-Boot, uploads, Wi-Fi, devshell) + every gotcha + the sideload-packaging plan |
| [hardware-specs.md](hardware-specs.md) | SoC, memory, flash layout, **free space**, **userland tool inventory**, peripherals, MACs, firmware versions |
| [uart-console.md](uart-console.md) | Physical access, baud (**locked to 115200**), the `uartctl.py` tooling, catching U-Boot |
| [boot-and-uboot.md](boot-and-uboot.md) | U-Boot environment, boot flow, readable verbose boot / root shell |
| [firmware-backup.md](firmware-backup.md) | How the full 16 MB flash was backed up (lua+RLE+base64 over UART) and verified |
| [usb-gadget-console.md](usb-gadget-console.md) | Using the USB port as a console/network gadget (no soldering) — validated substrate + plan |
| [ir-api-project.md](ir-api-project.md) | **The project:** expose IR over an HTTP/gRPC API — architecture, Go vs Rust, recovery |
| [logs/](logs/) | Raw captured console logs (reference artifacts) |

## TL;DR
- **Device:** Logitech Harmony Hub, codename **Pimento**.
- **SoC:** Atheros **AR9331 "Hornet"** (MIPS 24Kc), board **AP121**. 32 MB RAM, 16 MB NOR flash.
- **Console:** `/dev/cu.usbserial-A50285BI` @ **115200 8N1**, FTDI USB-TTL adapter.
- **Bootloader:** U-Boot 1.1.4 (prompt `ar7240>`, `bootdelay=0` — must flood Enter to interrupt).
- **OS:** Linux 2.6.31 + BusyBox 1.13.4 (Poky/Yocto build, Feb 2020).
- Stock boot has `console=none`; readable boot needs a `bootargs` override (see boot doc).
- **Backup:** full 16 MB flash captured & verified (whole-chip md5 `48154ae8…`) — see [firmware-backup.md](firmware-backup.md) and [`../backups/`](../backups/).

## Project goal & decisions
Turn the hub into an **IR blaster/receiver with a network API (HTTP/gRPC)**. Verified by
extracting the stock rootfs (`backups/mtd3.bin`). Key calls:
- **IR path (verified):** the stock HAL daemon (`/usr/bin/hal`) owns the `cc2544` IR chip via `/dev/rfspi`; it already exposes IR over **LTCP on `127.0.0.1:16716`** and a **LAN HTTP API on `:8088`**. Our service is an HTTP front end that **translates to LTCP / proxies `:8088`** — don't open the device directly.
- **Keep the stock 2.6.31 kernel + `cc2544.ko`** (locked to that kernel; OpenWrt would lose IR). Add a userland service; persist it on the jffs2 overlay via `/etc/init.d/rcS.local` — **no flashing**.
- **Language: Go is RULED OUT, Rust is VERIFIED** — tested on hardware. The kernel has **no `CONFIG_FUTEX`**, so Go's runtime crashes at startup (`futex ENOSYS`, all versions). A **single-threaded static-musl Rust** binary runs cleanly (TCP+UDP, exit 0) — **Rust is the chosen language** (keep it single-threaded). Build recipe in `service/build-mips.sh` (Rust 1.74 + bundled rust-lld, no Docker/gcc). Deploy **native** — UPX's stub SIGTRAPs on this kernel. (Lua, already on-device with luasocket, remains a no-cross-compile alternative.) See [ir-api-project.md](ir-api-project.md).
- **⚠️ Real blocker = networking:** only `lo` is up. Need **USB-ethernet gadget** (one cable = power + ssh console + API) or WiFi. See [usb-gadget-console.md](usb-gadget-console.md).
- **Recovery:** U-Boot serial recovery covers all realistic mistakes while mtd0 is intact; never erase mtd0 or mtd6/mfg. SPI programmer (CH341 *not* required) is last-resort only.

See [ir-api-project.md](ir-api-project.md) for the full verified plan.

> Dates in these docs are absolute. Gathered 2026-06-21.

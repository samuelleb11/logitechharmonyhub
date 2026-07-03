# Project: expose IR over a network API

**Goal:** turn the Harmony Hub into a standalone **IR blaster/receiver with a network
API** (HTTP and/or gRPC).

**Status (2026-06-21):** feasibility *verified* by static analysis of the backed-up
rootfs (a multi-agent workflow extracted `mtd3.bin` and read the real driver, daemon,
IPC, and init scripts). The IR path, the integration point, the language choice, and
the recovery story are all settled. **The one genuine blocker is networking** (see
below), not IR.

## Requirements (decided with the user, 2026-06-21)
- **Standalone appliance** — do NOT run the Harmony cloud/app (luaworks). Reuse the stock
  `/usr/bin/hal` daemon (we start it: `insmod cc2544.ko` → load `cc2544.bin` firmware → run
  `hal`) and drive IR via LTCP — not the raw `/dev/rfspi` protocol. (Confirm hal runs without
  luaworks; fall back to Route B only if not.)
- **Transport: Wi-Fi** onto the user's LAN (stock Atheros driver + `wpa_supplicant` + `udhcpc`).
  Credentials go in a config file on the device overlay, never in source/chat.
- **API: HTTP/JSON REST**, **open on the LAN** (no auth for v1), shaped **Home-Assistant-friendly**
  (maps to HA RESTful Command/Switch; MQTT a possible later add).
- **Features: send + learn (capture) + a device/code database** (persisted on the jffs2 overlay).
- **Language: single-threaded Rust**, static mips-musl, native deploy (no UPX). See Language section.
- **First target: a TV** (codes taught by pointing its real remote at the hub). **Default IR
  emitters: ALL** (built-in blaster + both mini-jack extender ports → `ports` bitmask = all).
- **Recovery rule unchanged:** overlay-only/reversible, never touch mtd0 (u-boot) or mtd6 (mfg).

## Verified architecture

```
LAN client ──HTTP/JSON──▶ [our Rust service, single-threaded static mips-musl]
                              │  (lives on the jffs2 overlay)
                              │  speaks LTCP framing → 127.0.0.1:16716
                              ▼
                       HAL daemon  /usr/bin/hal   (stock, unmodified)
                              │  HBus  /ir/ir_send , /ir/ir_cap
                              ▼
                /dev/rfspi (10,59)  +  /dev/rffw (10,60)   ← stock cc2544.ko
                              ▼
                       TI CC2544 (8051 RF SoC) → 6 IR emitters + RF remote
```

## The IR interface (verified from the rootfs)
- The IR/RF front end is a **TI CC2544** (a 2.4 GHz RF SoC with an 8051 core), driven
  by the AR9331 over bit-banged GPIO-SPI. It is **not** a Linux lirc/rc-core device.
- Driver: `lib/modules/2.6.31-g89d565c/cc2544.ko` (GPL, Logitech authors, **not
  stripped**). Loaded at boot via `/etc/modules`.
- Device nodes (static, in the squashfs `/dev`):
  - **`/dev/rfspi`** (char **10,59**, `cc2544_spi`) — main IR/RF data device.
  - **`/dev/rffw`** (char **10,60**, `cc2544_fw`) — firmware-load device.
- Firmware blob `lib/firmware/cc2544.bin` (20929 B, v39.00.0010) is pushed to the chip
  at boot: `cat /lib/firmware/cc2544.bin > /dev/rffw`. The CC2544 firmware does the IR
  **carrier modulation**; the host sends high-level command packets.
- **Mechanism = `read()`/`write()` of length-prefixed framed packets — there is NO
  ioctl** (`cc2544_spi_fops` exports only `open/read/write/release`). So any language
  with `open/read/write` is sufficient; the earlier "reverse the ioctl numbers" framing
  was wrong for this driver.
- **`/usr/bin/hal` (the HAL daemon) is the sole opener** of `/dev/rfspi`. It exposes IR
  via `libhal_ir_send` (TX) and `libhal_ir_cap_*` (RX/learning), with carrier-frequency
  + duty-cycle + raw-packet semantics.

## Two integration routes
**Route A — talk to the existing HAL (recommended, lowest risk).**
The `luaworks` app talks to HAL over **LTCP: JSON-ish commands on `127.0.0.1:16716`**
(custom framing, XOR checksum, `SERVICE_ID`, ops `open/seek/write/read/devctl/...`).
IR commands use HBus paths `/ir/ir_send` and `/ir/ir_cap`. **There is also already a
LAN HTTP/WebSocket API on `:8088`** (the well-known Harmony local API — `POST /?...`,
JSON, SSDP-advertised, nonce/hubId auth via `/etc/nonce`), whose registry
(`apilistener.lua`) includes IR-triggering commands like
`control.button?press|fire|hold|release`, `harmony.engine?startactivity`, etc.

→ Build our service as an **HTTP/JSON front end that translates to LTCP toward HAL**
(or simply proxy/extend the existing `:8088` API). This reuses Logitech's entire
firmware + carrier path and never opens the device directly. **Do not also open
`/dev/rfspi`** — HAL holds it (open is likely exclusive).

**Route B — drive `/dev/rfspi` directly (full control, higher risk).**
Write framed packets straight to the device after loading firmware. Requires reversing
the `rf_data_head` + payload-length framing (disassemble `cc2544_spi_read/write` in the
un-stripped `cc2544.ko` with Ghidra/MIPS objdump) and **conflicts with HAL** (stopping
HAL breaks the RF remote and the watchdog heartbeat `/var/watchdog/hal` → reboot).
Keep this as a future option only if HAL doesn't expose something you need.

## ⚠️ The real blocker: networking
Linux shows **only `lo`** — no IP connectivity, so an API server is useless until we
have a network path. This is the actual critical path, independent of IR and language.
Options:
- **USB-Ethernet gadget** — best fit: one USB cable = power + ssh console + the IR API.
  See [usb-gadget-console.md](usb-gadget-console.md). (Synergy: solves console *and*
  transport at once.)
- **WiFi** — `wpa_supplicant` + `ath_ahb` are present; associate to the LAN. Needs
  bring-up from the bare shell.

## Language: **Go is RULED OUT — pivot to single-threaded Rust / Lua / C**
This was settled empirically on hardware (2026-06-21), and it overrides the workflow's
earlier "Go decisively." A static big-endian-MIPS Go probe (`GOARCH=mips
GOMIPS=softfloat CGO_ENABLED=0`) was cross-compiled, uploaded over UART (md5-verified),
and **crashed at startup** in `runtime.futexwakeup` with `futex returned -89` (`-89` =
`ENOSYS` on MIPS). Confirmed at the kernel level via `/proc/kallsyms`: `do_futex` is
absent and `sys_futex`/`compat_sys_futex` both alias the `sys_ni_syscall` stub — i.e.
the kernel was built **without `CONFIG_FUTEX`**.

Go's Linux runtime spawns OS threads and coordinates them with `futex` before `main()`,
so **no Go version can run here** (Go 1.17 doesn't help; it wasn't a UPX issue either —
the non-UPX binary gave the same crash with a full stack trace). The only way to enable
Go would be to rebuild the kernel with `CONFIG_FUTEX=y`, which risks the `cc2544.ko`
vermagic lock — not worth it. (Device libc is **uClibc 0.9.30.1** using LinuxThreads,
which is why the stock system works without futex.)

The pivot, in order of pragmatism:

- **Lua (cleanest, on-device).** Lua 5.1 is already installed; luasocket `.so` +
  `/opt/lib/luaworks.so` (IR codec) exist under `/opt` (just set `package.cpath`). The
  stock `:8088` API is *already* Lua doing HTTP→LTCP→HAL→IR. No cross-compile, no binary
  upload, no futex — extend/mirror `apilistener.lua`. Best speed-to-working.
- **Rust, single-threaded — ✅ VERIFIED ON HARDWARE (the chosen language).** A
  single-threaded static-musl big-endian-MIPS probe ran cleanly on the device
  (`RUST_START → TCP_LISTEN_OK → UDP round-trip → RUST_RUNS_2631_OK`, exit 0) — no futex
  crash. Rust std only calls `futex` for `thread::spawn`/join and *contended* std locks
  (an uncontended `Mutex` is a lock-free atomic fast path, no syscall), so a
  **single-threaded** service never touches it. **Keep it single-threaded.** Build recipe
  (Rust 1.74 — last Tier-2 mips-musl with prebuilt self-contained std; bundled `rust-lld`;
  no Docker, no external gcc) is in `service/build-mips.sh` + `service/rustprobe/.cargo/config.toml`.
  Native binary ~547 KB (`opt-level=z`+lto+`panic=abort`+strip).
- **C, static musl, single-threaded** — the safest fallback; smallest; full control.

### Binary size / upload notes
- **Native Rust probe: ~547 KB** static (fits the ~4 MB jffs2 overlay easily; demand-paged,
  so native is *better* for the 32 MB RAM than a compressed blob). For Go (now moot): 2.29 MB
  net / 1.64 MB no-net.
- **⚠️ Do NOT UPX — deploy native.** UPX packs fine on big-endian MIPS (Rust 547 KB → 194 KB),
  but its self-extracting stub **SIGTRAPs at runtime** on this 2.6.31 kernel (confirmed: native
  Rust runs, UPX'd Rust crashes identically to UPX'd Go). Smaller *native* binaries would need
  nightly `-Z build-std` w/ `panic_immediate_abort` — a future ~2× win.
- **Upload pipeline that works** (the device tty is cooked-mode, ~1024-byte canonical
  limit, echo on, and `/bin/sh` is PID 1 so a truncated *quoted* paste wedges the shell):
  `zip -9` the binary (BusyBox has `unzip`, not gzip) → base64 on host →
  `tools/upload_file.py <b64> <devpath> 800` (chunked **unquoted** `printf %s … >> file`
  with a per-chunk sentinel handshake) → `lua tools/b64decode.lua` on device → `unzip` →
  md5-verify. Probe source: `service/probe/main.go`.

## Deployment — no flashing
`/` is unionfs(rw) over the jffs2 `data` partition + squashfs root, so our binary +
init hook are **plain files that persist without flashing**:
1. Cross-compile the static mipsbe binary on the Mac; verify `file` says **MSB +
   soft-float**.
2. Get it onto the device (delivery logistics: no `scp`/`wget`/`nc` — use the same
   base64-over-UART channel as the backup, or U-Boot `loady` into RAM → save to jffs2).
3. Place it in `/data/` (persists on jffs2); `chmod +x`.
4. Hook it from **`/etc/init.d/rcS.local`** (sourced by `rcS` at ~line 42, after module
   load, before HAL/luaworks): `[ -x /data/irapi ] && /data/irapi &`.
5. Reboot — persists, no flash touched, reversible by deleting the file.

**Watchdog caveat:** `/usr/bin/watchdog` reboots on missed heartbeats; don't block or
crash `hal`/`luaworks`, and keep RAM use lean (OOM → watchdog reboot).

## Recovery matrix (verified) — you do NOT need the CH341 for this project
- **Primary:** jffs2-overlay deploy = nothing to recover (delete the file, reboot).
- **Bad kernel:** the dual-kernel `bootcmd` (`bootm 0x9f010000 ; bootm 0x9f100000`)
  auto-falls back to **mtd2/kernel2** if mtd1's uImage CRC is bad. Experiment only on
  **mtd1**, keep **mtd2** as the known-good rescue kernel.
- **Restore any of mtd1–mtd5** from the verified `backups/*.bin` over U-Boot serial:
  ```
  ar7240> loady 0x80060000              # YMODEM the image into RAM (115200)
  ar7240> erase 0x9f010000 +0xF0000     # mtd1 (use 0x9f100000 for mtd2)
  ar7240> cp.b 0x80060000 0x9f010000 ${filesize}
  ```
  (`flinfo` first; erase only within the intended partition.)
- **NEVER erase/overwrite `mtd0` (u-boot) or `mtd6` (mfg/cal, per-unit, irreplaceable).**
  As long as mtd0 boots to `ar7240>`, serial recovery covers everything — no programmer.
- **SPI programmer = last resort only** (dead U-Boot, or mtd6 damage with no console).
  The CH341 is **not** required: `flashrom` supports FT2232H/Tigard (`-p ft2232_spi`),
  Raspberry Pi `spidev`, Bus Pirate. The software-reported `0x100000ff` JEDEC id is a
  **fake** Atheros fallback — read the real part off the chip's top marking (SOIC-8
  208mil or WSON-8; likely MX25L12835 / W25Q128 / GD25Q128 / S25FL128). Match the clip
  to the package, or solder flying leads, or chip-off into a socket. **In-circuit work
  must hold the SoC in reset** — confirmed the cc2544 IR driver *also* drives the SoC
  SPI bus (`ar7240_flash_spi_up/down`), so it shares the bus with the NOR flash.

## Concrete next experiments (do in order)
1. ~~"Does Go run?"~~ no (futex). ~~"Does single-threaded Rust run?"~~ **DONE — yes**
   (verified: TCP+UDP, exit 0; recipe in `service/build-mips.sh`). **Language settled: Rust,
   single-threaded.** Next: write the real service.
2. **LTCP `ir.send` schema (Route A).** Extract the JSON field set (`code` encoding,
   `keyLatency`, `ports`, `mode`, `repeat`, carrier/duty) from `irsender.lua` + the
   `libir.c` paths in `hal`, or sniff loopback to `:16716` while pressing a button via
   the `:8088` API. Confirm one IR blast end-to-end *through HAL* before writing the
   service.
3. **Networking bring-up** (USB-ethernet gadget or WiFi) — gating the whole goal.
4. *(Route B only)* reverse `rf_data_head` framing + test `/dev/rfspi` single-open.

## Open unknowns
- [x] ~~Go on 2.6.31~~ — RESOLVED: **no** (kernel has no futex).
- [x] ~~single-threaded Rust runs futex-free on 2.6.31~~ — RESOLVED: **yes** (verified; Rust chosen).
- [ ] (blocking the goal) network/IP bring-up — no validated path yet.
- [ ] LTCP `ir.send` JSON schema (for Route A).
- [ ] `/dev/rfspi` packet framing + single-open (only for Route B).
- [ ] mtd4 free space vs. Go binary size — measure; fall back to tmpfs/`/cache`.
- [ ] physical flash part/package — only matters if a programmer is ever needed.

## Reference artifacts
- Reusable squashfs extractor (stock uses a legacy Atheros LZMA1 variant that mainline
  `unsquashfs` can't read): [`../backups/sqextract-atheros-lzma.py`](../backups/sqextract-atheros-lzma.py)
- Full rootfs file/perm/devnode manifest: [`../backups/mtd3-rootfs-manifest.txt`](../backups/mtd3-rootfs-manifest.txt)
- Re-extract anytime: `python3 backups/sqextract-atheros-lzma.py backups/mtd3.bin <outdir>`
- Key stock files (in an extracted rootfs): `usr/bin/hal`, `lib/firmware/cc2544.bin`,
  `lib/modules/2.6.31-g89d565c/cc2544.ko`, `opt/luaworks/tasks/harmonyengine/core/irsender.lua`,
  `opt/luaworks/tasks/hal/core/{hbus,ltcp}.lua`, `opt/luaworks/tasks/apilistener.lua`,
  `etc/init.d/rcS.local`.

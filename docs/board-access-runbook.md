# Board Access & Setup Runbook

**Goal:** take a powered-off (even cased) Logitech Harmony Hub to an **untethered root
shell over Wi-Fi**, ready to sideload our software. This is the proven end-to-end
procedure (verified 2026-07-02), the tools it uses, and every gotcha we hit.

> This is the basis for a future one-command "sideload" package (see §9). Every step
> below is scripted or scriptable; nothing here is manual-only.

---

## 0. Prerequisites

**Hardware**
- FTDI USB-TTL adapter (**3.3 V**) wired to the **J2** pad cluster: **pad 2 = RX**, **pad 4 = TX**,
  **pad 8 = GND** (adapter TX→pad 2, adapter RX→pad 4, GND→pad 8). **115200 8N1**, enumerating as
  `/dev/cu.usbserial-*` on macOS. See [uart-console.md](uart-console.md).
- The hub's **USB port** cabled to the host = optional (gadget-mode; not used here).

**Host tools** (macOS)
- `python3` + `pyserial` (the serial tooling)
- `java` (for `unluac` Lua-bytecode decompiling — protocol RE only)
- Rust **1.74** with `mips-unknown-linux-musl` (see `rust-mips-toolchain` memory / `service/build-mips.sh`)
- `zip`, `base64`, `nc`

**Repo tools** (all in this repo)
| tool | purpose |
|------|---------|
| `uartctl.py` | serial console logger daemon + control CLI (owns the port) |
| `catch_uboot.py` | flood Enter to interrupt U-Boot autoboot |
| `tools/upload_file.py` | reliable binary→device transfer over the cooked-mode UART |
| `tools/b64decode.lua` | on-device base64 decoder (device has no `base64`) |
| `tools/dumpflash_rle.lua` + `tools/extract_rle64.py` | full flash backup over UART |
| `service/build-mips.sh` | cross-build static big-endian-MIPS Rust binaries |
| `service/standalone-ir-bringup.sh` | one-shot IR + hal + Wi-Fi + service bring-up |
| `service/irapi/` | our Rust binary (IR service **and** `devshell` untethered shell) |

---

## 1. Serial console — `uartctl.py`

One daemon owns the port and logs every byte; you send through a FIFO so logging is
never interrupted. It auto-reconnects across device power cycles.

```
python3 uartctl.py start                 # start (cu.* @ 115200)
python3 uartctl.py run --timeout N 'cmd' # send a shell command, return just its output
python3 uartctl.py send 'text'           # send a line + CR
python3 uartctl.py spam '\r' 90          # flood a key for N s (catch U-Boot)
python3 uartctl.py dump 800              # last N bytes of the log
python3 uartctl.py wait 'regex' T        # block until regex appears
python3 uartctl.py stop
```
Log: `/tmp/uart-<ts>.log` (+ symlink `/tmp/uart-latest.log`).

> **GOTCHA — one owner:** only one process may own the tty. `screen`/`cu` will report
> "Resource busy" while the daemon runs. Stop it first. If `status` says NOT RUNNING but
> the port is busy, an **orphaned daemon** is holding it (our many `restart` cycles can
> leave one): `lsof /dev/cu.usbserial-* ` → `kill <pid>`.

---

## 2. Catch U-Boot → root shell

The bootloader autoboots with `bootdelay=0` (instant), so you **flood Enter** to
interrupt it. Boot to a bare shell with a RAM-only bootargs override (reverts on next
power cycle — keeps the device recoverable; never `saveenv`).

```
python3 uartctl.py start                                    # daemon at 115200
python3 uartctl.py spam '\r' 120 &                          # flood (background)
#   >>> POWER-CYCLE the hub now <<<
python3 uartctl.py wait 'ar7240>' 120                       # U-Boot prompt caught
pkill -f "uartctl.py spam"
python3 uartctl.py send 'setenv bootargs console=ttyS0,115200 init=/bin/sh'
python3 uartctl.py send 'bootm 0x9f010000'
python3 uartctl.py wait 'BusyBox v1.13' 30                  # -> `/ #` shell
```

> **GOTCHAs:**
> - **Console is locked to 115200** — the kernel ignores higher `console=` bauds (see `uart-console.md#baud-lock`).
> - **FTDI drops on target power-on:** applying power to the hub can make the FTDI
>   briefly re-enumerate on the host, so the flood misses the (instant) autoboot window —
>   you may need 2–3 tries. Watch the log: if you see clean `U-Boot 1.1.4` text but no
>   `ar7240>`, the flood just missed; retry. If you see *nothing*, the adapter dropped.
> - **Never type `$(( ))` (arithmetic) to the device shell.** A truncated `$((` (or a
>   truncated single-quoted string) leaves the shell in PS2 continuation, and since
>   `/bin/sh` is PID 1, **Ctrl-C is ignored** — recovery means closing the open construct
>   (`))`/`'`/`"` + newline). Prefer plain commands.

---

## 3. Upload binaries over UART (`tools/upload_file.py`)

The device has **no `dd`/`nc`/`scp`/`wget`/`base64`**, the tty is cooked-mode with echo,
and `/bin/sh` is PID 1 — so naive streaming corrupts or wedges. The working pipeline:

```
# host: compress (BusyBox has `unzip`, not gzip) + base64
zip -9 -j /tmp/x.zip mybinary
base64 < /tmp/x.zip > /tmp/x.zip.b64

# one-time: upload the lua decoder (device has no base64)
python3 uartctl.py send 'cat > /tmp/b64d.lua'
python3 uartctl.py send "$(cat tools/b64decode.lua)"
python3 uartctl.py key ctrl-d

# stream the base64 in ≤800-char UNQUOTED `printf … >> file` chunks, handshaked per chunk
python3 tools/upload_file.py /tmp/x.zip.b64 /tmp/x.zip.b64 800

# device: decode → unzip → verify md5 against host
python3 uartctl.py run 'lua /tmp/b64d.lua /tmp/x.zip.b64 /tmp/x.zip; unzip -o /tmp/x.zip -d /tmp; md5sum /tmp/x.bin'
```

> **GOTCHAs:**
> - **tty canonical limit ≈ 1024 bytes/line** — chunks must stay well under it (we use 800).
> - **Chunks are UNQUOTED `printf %s`** so a truncated line can't leave an open quote and wedge the shell.
> - **`/tmp` is the PERSISTENT jffs2 overlay under `init=/bin/sh`** (because `mount -a`
>   never ran → no tmpfs on `/var/volatile`). Uploaded files **survive reboot** and can
>   **fill the ~5 MB partition** ("No space left on device"). Clean `/tmp`, or put a real
>   tmpfs (`mount -t tmpfs tmpfs /var/volatile`), or use `/cache` (separate 5 MB jffs2).
> - **Pause any CPU-heavy process** (e.g. `hal`) during upload — contention causes handshake timeouts.
> - Persistent install location: **`/root/…`** or `/data/…` (on the overlay; survives a
>   normal reboot even when `/tmp` becomes tmpfs).

---

## 4. Wi-Fi bring-up (station on a WPA2 network)

Load the Atheros stack, make a station VAP, associate, DHCP.

```
# credentials: service/ssh/wpa_supplicant.conf (gitignored) -> device /etc/wifi/wpa_supplicant.conf
M=/lib/modules/2.6.31-g89d565c
for m in asf adf ath_hal ath_rate_atheros ath_dev umac; do insmod $M/$m.ko; done   # creates wifi0
wlanconfig ath0 create wlandev wifi0 wlanmode sta                                   # creates ath0
ifconfig ath0 up
wpa_supplicant -B -Dmadwifi -iath0 -c /etc/wifi/wpa_supplicant.conf                 # associate
wpa_cli -i ath0 status | grep wpa_state          # wait for COMPLETED
udhcpc -i ath0 -b -t 6 -s /usr/share/udhcpc/default.script                          # get IP
ifconfig ath0 | grep 'inet addr'
```

> **GOTCHAs:**
> - **Driver is `madwifi`, NOT `wext`** (wext is "Unsupported" in this wpa_supplicant build). Only `madwifi` works.
> - **Bring up `lo` first** for anything binding 127.0.0.1 (hal, our services): `ifconfig lo 127.0.0.1 up`.
> - Interface tiers: `wifi0` (radio) → `ath0` (station VAP). Never `wlan0`.
> - Regulatory/calibration data is auto-read from **mtd6** by `ath_hal` at insmod — do NOT overwrite mtd6.

---

## 5. Untethered shell (no dropbear)

**`dropbear` was stripped from this production build** (only `/etc/dropbear/` + a host
key remain; `luasocket` is entangled in `luaworks.so` and segfaults standalone). So we
ship our own: **`irapi devshell`** — a single-threaded, token-authed TCP command shell
(single-threaded because the kernel has **no `CONFIG_FUTEX`**; see the `harmony-no-futex-no-go` memory).

```
# on device (after §3 upload to /root/irapi):
/root/irapi devshell --port 2222 --token <secret> &

# from the host, over Wi-Fi — NO UART:
printf '<secret>\nid\nuname -a\nexit\n' | nc <hub-ip> 2222
```
Verified: `id` → `uid=0(root)` over the network. (Dev tool on a trusted LAN; not hardened
SSH. A static `dropbear`/`openssh` could be cross-built later if real SSH is wanted.)

---

## 6. IR stack (for the actual project)

To make IR work, run `service/standalone-ir-bringup.sh` (or these stages): loads
`cc2544.ko` + firmware, brings up `lo` + dbus, starts `hal` (`-f -s`) which then listens
on `127.0.0.1:16716` (LTCP). See `ir-service-buildplan.md` for the protocol and `irapi`.

> Critical: hal needs **`ifconfig lo up`** (binds loopback) + **dbus running** or it exits
> silently with `libhal_run: ERROR on binding: Cannot assign requested address`.

---

## 7. Full flash backup (do this before writing anything)

`tools/dumpflash_rle.lua` + `tools/extract_rle64.py` dump all 16 MB over UART, md5-verified.
See `firmware-backup.md`. Backups live in `backups/` (whole-chip md5 cross-checked). This
is the safety net; **never erase mtd0 (u-boot) or mtd6 (mfg/cal)**.

---

## 8. Recovery

- While **U-Boot (mtd0) is intact**, you can always recover over serial (`loadb`/`loady` →
  `erase` → `cp.b`) — no external programmer needed. Dual-kernel fallback: `bootcmd` auto-boots
  mtd2 if mtd1 is bad. Restore any of mtd1–mtd5 from `backups/*.bin`.
- **Never touch mtd0 or mtd6.** An external SPI programmer is only for a dead U-Boot.

---

## 9. Persistence — DEPLOYED & VERIFIED (2026-07-02)

The device now comes up **standalone on Wi-Fi with the devshell, on a normal power-on,
with NO U-Boot catch.** Mechanism: an **overlay `/etc/init.d/rcS.local`** (repo copy:
`service/rcS.local`), which stock `rcS` `source`s at line ~43 (after `mount -a`, module
load, `ifup lo`, but before dbus/hal/wifi/luaworks). Ours does the full bring-up and
**`exit 0`** — since it's *sourced*, that stops `rcS`, so **luaworks/luadraws never start**
(standalone). Verified: plain reboot → `nc 10.30.0.197:2222` (token `harmonydev`) → root.

What `rcS.local` starts, in order:
1. **UART fallback shell:** `getty -n -l /bin/sh -L ttyS0 115200 &` — the SAFETY NET. Works
   even with `console=none`, and `ttyS0` is free because we skip luaworks (see below). So a
   Wi-Fi failure never locks you out — just attach the FTDI and you get a clean `/ #`.
2. cc2544 firmware + dbus + `ifconfig lo up` + `hal -f -s` (IR ready on `:16716`).
3. Wi-Fi: try **station** (madwifi + `udhcpc`); if no config or it can't get an IP, fall
   back to our **own open AP** — `wlanconfig ath1 … wlanmode ap`, SSID `Harmony-Setup`,
   `192.168.4.1`, plus `irapi dhcpd` (a DHCP server we wrote — device has no udhcpd).
   Verified end-to-end: a phone joins `Harmony-Setup` and gets `192.168.4.10`.
4. `/root/irapi devshell --port 2222 --token harmonydev` (untethered access).

> **KEY FINDING: `ttyS0` is the Bluetooth transport.** Stock `rcS.local` ends with
> `hciattach ttyS0 bcsp` (CSR8811 BT over BCSP) — *that* is the binary "pollution" seen on
> the console when the app runs, not app noise. Because our standalone `rcS.local` replaces
> the stock one and skips `hciattach`, `ttyS0` stays a clean console (at the cost of BT,
> which we don't need).

**Operating it:**
- Normal use: `nc <hub-ip> 2222` → token → root shell. Or `scp`-less file push still via UART.
- Find the IP after reboot: it kept its DHCP lease (`10.30.0.197`); otherwise check the
  router or ping-sweep the subnet for an open `:2222`.
- **Revert to stock:** delete the overlay file — `rm /etc/init.d/rcS.local` (via devshell or
  UART), reboot. Nothing else was changed; no flash partition was touched.
- Files (all persistent, on the jffs2 overlay): `/etc/init.d/rcS.local`, `/root/irapi`,
  `/etc/wifi/wpa_supplicant.conf` (mode 600).

## 10. Remaining packaging (the "one command" goal)

- **`sideload.sh` (host):** wrap §1–§5 + §9 — start daemon, guide the (one-time) U-Boot
  catch, upload `irapi` + `wpa_supplicant.conf` + `rcS.local` via `upload_file.py`, verify,
  done. After that first install, the board is self-sufficient over Wi-Fi.
- **Real SSH (optional):** the devshell is a plaintext dev tool; a static `dropbear`/OpenSSH
  would need a mips-musl **C** cross-toolchain (we only have Rust). Deferred.

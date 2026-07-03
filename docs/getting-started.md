# Getting Started — Standalone Harmony IR Appliance

Turn a Logitech Harmony Hub (Atheros AR9331) into a **standalone network IR blaster/receiver**
— no Logitech cloud, no MyHarmony app, no account. You flash nothing: the hub keeps its stock
kernel and boots our software off the jffs2 overlay via a single reversible file. When it's done
you get a web UI at `http://<hub-ip>/`, an HTTP/JSON API, and a Home Assistant integration.

> **How it works in one line:** we install our Rust binary `irapi` to `/root`, a code database to
> `/cache`, and an overlay `/etc/init.d/rcS.local` that boots the appliance instead of Logitech's
> app. `irapi` drives the AR9331 **I2S** peripheral directly to transmit and receive IR — Logitech's
> `hal` is not used. Delete the one overlay file to revert to stock.

This guide assumes you're comfortable with a serial console and a terminal. Read the linked docs —
[`board-access-runbook.md`](board-access-runbook.md) and [`firmware-backup.md`](firmware-backup.md) —
before touching hardware.

---

## 1. What you'll need

**Hardware**
- The **Logitech Harmony Hub** (codename *Pimento*; AR9331, Linux 2.6.31, 16 MB NOR, 32 MB RAM).
- A **3.3 V USB-UART adapter** (FTDI or similar), wired to the hub's UART pads: **RX / TX / GND, 115200 8N1, 3.3 V logic**. On macOS it enumerates as `/dev/cu.usbserial-*`. Pinout and photos: [`uart-console.md`](uart-console.md).
  > ⚠️ 3.3 V logic only. A 5 V adapter can damage the SoC.
- No soldering iron strictly required if your pads are already broken out, but most units need header pins tacked on.

**Host software** (macOS shown; Linux is equivalent)
- `python3` + `pyserial` — the serial tooling (`tools/uartctl.py`, `tools/upload_file.py`).
- `zip`, `base64` — used by the UART upload pipeline (the device has no `gzip`/`base64`).
- **Rust 1.74** with the `mips-unknown-linux-musl` target — to cross-build `irapi`. This is the last
  Rust that ships a prebuilt std for big-endian MIPS musl; 1.75+ demoted it to Tier 3. One-time setup
  (see the header of [`../service/build-mips.sh`](../service/build-mips.sh)):

  ```sh
  brew install rustup zip
  rustup toolchain install 1.74.0 --profile minimal
  rustup target add mips-unknown-linux-musl --toolchain 1.74.0
  ```

  > Do **not** use UPX and do **not** try Go. UPX's stub SIGTRAPs on this kernel, and the kernel has
  > **no `CONFIG_FUTEX`** so any multi-threaded runtime (Go, async Rust) crashes at startup. `irapi` is
  > deliberately single-threaded, statically linked, zero runtime crates — deploy the native binary.

---

## 2. SAFETY — back up the flash first

**Before you write a single byte to the hub, capture a full 16 MB flash backup.** It's your only
recovery net, and part of it is *irreplaceable*.

- Follow [`firmware-backup.md`](firmware-backup.md). It dumps all 7 partitions over the UART using
  on-device `lua` (RLE + base64), and **md5-verifies each partition against the device**. One command
  does the whole chip:

  ```sh
  tools/backup_all.sh
  ```

  Backups land in [`../backups/`](../backups/), whole-chip md5 cross-checked.

- **NEVER erase `mtd0` (U-Boot) or `mtd6` (per-unit mfg/calibration).**
  - `mtd0` is the bootloader — while it's intact you can always recover over serial with no external
    programmer. Erase it and you need an SPI flasher.
  - `mtd6` holds this unit's **radio calibration + MAC/identity**. It is per-unit and **cannot be
    restored from another hub's backup**. `ath_hal` reads it at Wi-Fi module load; overwrite it and the
    radio is permanently degraded.

  Everything we install lives on the jffs2 overlay (files, not partitions) — no `mtd` partition is ever
  written by this project. Recovery detail: [`board-access-runbook.md` §8](board-access-runbook.md).

---

## 3. Build the firmware

From the repo root, cross-compile `irapi` for the hub:

```sh
bash service/build-mips.sh service/irapi
```

Output (a ~580 KB, static, **MSB MIPS** ELF):

```
service/irapi/target/mips-unknown-linux-musl/release/irapi
```

`file` on it should report `ELF 32-bit MSB executable, MIPS ... statically linked`. That single binary
is the whole appliance — it dispatches on `argv[1]`: `serve` (web UI + API), `fire`, `ac`, `learn`,
`devshell`, `dhcpd`, plus hardware/RE subcommands. Run it with no args to see the usage.

You'll also deploy the offline code database, which is already in the repo:

```
service/irapi/codes/irdb.txt      # ~500-device curated Flipper-IRDB subset (CC0), ~1.6 MB
```

---

## 4. First install — get onto the hub over UART

This is the **one-time** bootstrap. After it, the hub is self-sufficient over Wi-Fi and every later
update goes over the network (§7). Full background and every gotcha: [`board-access-runbook.md`](board-access-runbook.md).

### 4.1 Open the serial console and catch U-Boot

One daemon owns the tty and logs every byte; you send through a FIFO.

```sh
python3 tools/uartctl.py start                       # opens cu.usbserial-* @ 115200
python3 tools/uartctl.py spam '\r' 120 &             # flood Enter to interrupt autoboot
#   >>> POWER-CYCLE the hub now <<<
python3 tools/uartctl.py wait 'ar7240>' 120          # U-Boot prompt caught
pkill -f "uartctl.py spam"
```

Boot to a bare shell with a **RAM-only** bootargs override (reverts on next power cycle — never
`saveenv`):

```sh
python3 tools/uartctl.py send 'setenv bootargs console=ttyS0,115200 init=/bin/sh'
python3 tools/uartctl.py send 'bootm 0x9f010000'
python3 tools/uartctl.py wait 'BusyBox v1.13' 30     # -> a root `/ #` shell
```

> `bootdelay=0`, so the FTDI can miss the instant autoboot window — expect 2–3 tries. If you see clean
> `U-Boot 1.1.4` text but no `ar7240>`, the flood just missed; retry. Details + gotchas:
> [`board-access-runbook.md` §2](board-access-runbook.md).

### 4.2 Upload the base64 decoder (one time)

The device has no `base64`/`dd`/`nc`/`scp`, so we use its `lua` to decode. Install the tiny decoder
once:

```sh
python3 tools/uartctl.py send 'cat > /root/b64d.lua'
python3 tools/uartctl.py send "$(cat tools/b64decode.lua)"
python3 tools/uartctl.py key ctrl-d
```

### 4.3 Upload `irapi` to `/root`

`zip` compresses it (the device has BusyBox `unzip`, not `gzip`), then we stream the base64 in small
handshaked chunks and unpack on-device:

```sh
# HOST: compress + base64 the freshly built binary
zip -9 -j /tmp/irapi.zip service/irapi/target/mips-unknown-linux-musl/release/irapi
base64 < /tmp/irapi.zip > /tmp/irapi.zip.b64

# stream it up in <=800-char chunks (per-chunk handshake), then decode + unzip on-device
python3 tools/upload_file.py /tmp/irapi.zip.b64 /root/irapi.zip.b64 800
python3 tools/uartctl.py run 'lua /root/b64d.lua /root/irapi.zip.b64 /root/irapi.zip; unzip -o /root/irapi.zip -d /root; chmod +x /root/irapi; md5sum /root/irapi'
```

Compare that `md5sum` against the host:

```sh
md5 service/irapi/target/mips-unknown-linux-musl/release/irapi   # macOS
```

### 4.4 Install the standalone boot script

[`../service/rcS.local`](../service/rcS.local) is the one file that makes the hub an appliance. Stock
`rcS` `source`s it and our script does the bring-up then `exit 0`, so Logitech's `luaworks` app never
starts. It's small text — paste it directly:

```sh
python3 tools/uartctl.py send 'cat > /etc/init.d/rcS.local'
python3 tools/uartctl.py send "$(cat service/rcS.local)"
python3 tools/uartctl.py key ctrl-d
python3 tools/uartctl.py run 'chmod +x /etc/init.d/rcS.local; ls -l /etc/init.d/rcS.local'
```

> `rcS` only sources it if it's executable — the `chmod +x` is required.

### 4.5 (Optional) Seed the code database now

You can push the ~1.6 MB `irdb.txt` over UART, but it's slow. **Recommended: skip it here** — `irapi`
falls back to a small built-in default DB, so the web UI comes up fine, and you'll push the full DB
fast over HTTP in §7. If you'd rather do it over UART now:

```sh
zip -9 -j /tmp/irdb.zip service/irapi/codes/irdb.txt
base64 < /tmp/irdb.zip > /tmp/irdb.zip.b64
python3 tools/upload_file.py /tmp/irdb.zip.b64 /cache/irdb.zip.b64 800
python3 tools/uartctl.py run 'lua /root/b64d.lua /cache/irdb.zip.b64 /cache/irdb.zip; unzip -o /cache/irdb.zip -d /cache; ls -l /cache/irdb.txt'
```

> The DB lives on `/cache` (a separate 5 MB jffs2 partition) so the binary stays small. `irapi` loads
> `/cache/irdb.txt`, then `/root/irdb.txt`, then the embedded default.

### 4.6 Reboot into the appliance

```sh
python3 tools/uartctl.py send 'reboot'
```

On the **normal** power-on that follows, `rcS.local` brings up IR, Wi-Fi (or the setup AP), the web UI
on `:80`, and a token-authed dev shell on `:2222` — **no U-Boot catch needed ever again**. As a safety
net it also starts a passwordless login shell on the UART (`ttyS0`), so a Wi-Fi failure can never lock
you out: just attach the FTDI for a clean `/ #`.

---

## 5. Connect it to Wi-Fi

The hub is headless, so it uses an **AP-fallback** model: if it can't join a network it hosts its own.

### Option A — Setup web UI (AP fallback)

1. If no valid Wi-Fi config is present (or it can't get an IP), on boot the hub opens an **open access
   point** named **`Harmony-Setup`** at **`192.168.4.1`** (it runs its own DHCP server for you).
2. Join `Harmony-Setup` from a phone/laptop and browse to **`http://192.168.4.1/`**.
3. In the **Wi-Fi** panel: **Scan networks**, pick yours, enter the password, **Save & connect**.
4. The hub writes `/etc/wifi/wpa_supplicant.conf` and reboots to join your LAN as a station. Find its
   new IP from your router (or ping-sweep for an open `:2222`).

### Option B — Pre-seed `wpa_supplicant.conf`

Provide credentials up front (e.g. during the §4 UART session). Copy the example, fill in SSID + PSK:

```sh
cp service/ssh/wpa_supplicant.conf.example /tmp/wpa.conf
$EDITOR /tmp/wpa.conf                                  # set ssid= and psk=
```

Then install it to the hub with mode `600`:

```sh
python3 tools/uartctl.py send 'mkdir -p /etc/wifi; cat > /etc/wifi/wpa_supplicant.conf'
python3 tools/uartctl.py send "$(cat /tmp/wpa.conf)"
python3 tools/uartctl.py key ctrl-d
python3 tools/uartctl.py run 'chmod 600 /etc/wifi/wpa_supplicant.conf'
```

> This build's `wpa_supplicant` uses the **madwifi** driver (not `wext`/`nl80211`) and only supports
> WPA2-PSK — the example file is already set up for that. On boot `rcS.local` tries station mode first
> and only falls back to the `Harmony-Setup` AP if it can't associate or get a DHCP lease.

---

## 6. Everyday use — the web UI

Point a browser at **`http://<hub-ip>/`** (or `http://192.168.4.1/` in AP mode). It's a single
self-contained page served by `irapi serve` on port 80, with these panels:

| Panel | What it does |
|-------|--------------|
| **Status** | Wi-Fi mode/SSID/IP, IR readiness, uptime, firmware version. |
| **Remote** | Browse the offline library **type → brand → device → model**, then tap a function name to **fire** it. |
| **Air Conditioner** | Midea/Danby thermostat — power, mode (cool/heat/dry/fan/auto), fan, temp (17–31 °C), encoded live on-device. No DB entry needed. |
| **Manual command** | Blast an arbitrary **raw mark/space µs** sequence with a chosen carrier (advanced). |
| **Learn** | Capture a button from a **physical remote** (aim it at the hub front, press within ~15 s), then **Save** it as a named custom device — browsable/fireable like any library code. |
| **Wi-Fi** | Scan and join a network (writes the config and reboots). |
| **Update (OTA)** | Flash new firmware or replace the code database from the browser. |
| **Maintenance** | Reboot, or **Factory reset** (drops Wi-Fi + learned codes and reboots into the setup AP; keeps the appliance itself). |

The same actions are available as an **HTTP/JSON API** (used by Home Assistant):

```
GET  /api/status
GET  /api/ir/{types,brands,devices,functions}
POST /api/ir/send        {"device":"tv_samsung_...","function":"Power","select":7}
POST /api/ir/send        {"raw_us":[9000,4500,...],"carrier":38000,"duty":33}
POST /api/ac/send        {"power":"on","mode":"cool","fan":"auto","temp":22}
POST /api/ir/learn       {"timeout_ms":15000}
POST /api/ir/learn/save  {"function":"Power","carrier":38000,"us":[...]}
POST /api/ir/forget      {"device":"bedroom_tv","function":"Power"}
```

Example — fire a library code with `curl`:

```sh
curl -sX POST http://<hub-ip>/api/ir/send \
  -H 'Content-Type: application/json' \
  -d '{"device":"tv_samsung_samsung","function":"Power"}'
```

> Browse exact device ids and function names in the **Remote** panel, or via
> `GET /api/ir/devices` / `GET /api/ir/functions?device=<id>`.

---

## 7. Updating later — `tools/ota.py`

Once the hub is on the network, updates go over HTTP — no UART, no devshell. This is why we recommended
skipping the slow DB upload in §4.5.

Push the full code database (hot-reloaded, no reboot):

```sh
python3 tools/ota.py <hub-ip> db service/irapi/codes/irdb.txt <token>
```

Flash a rebuilt firmware (keeps the previous binary at `/root/irapi.prev`, then reboots):

```sh
bash service/build-mips.sh service/irapi
python3 tools/ota.py <hub-ip> firmware service/irapi/target/mips-unknown-linux-musl/release/irapi <token>
```

> The firmware OTA reboots the hub, so the HTTP connection dropping after the reply is normal — `ota.py`
> says so. Both endpoints require the token (§9); omit it and `ota.py` uses the insecure default.

---

## 8. Add it to Home Assistant

A UI-configured custom integration (**not** an add-on) talks to the appliance over your LAN. Full
instructions, services, and examples: [`../homeassistant/README.md`](../homeassistant/README.md).

Quick version:

1. **HACS → Custom repositories**, add the integration repo as category **Integration**, install
   **Harmony IR Blaster**, restart HA. (Or copy `custom_components/harmony_ir/` in manually.)
2. **Settings → Devices & Services → Add Integration → Harmony IR Blaster**, enter the hub IP.
3. In the integration's **Configure** menu, add library devices (each function becomes a button), and
   optionally enable the **Midea/Danby AC** climate entity.

You get a `remote` entity (fire / `remote.learn_command` / `remote.delete_command`), per-function
buttons, the optional AC thermostat, diagnostic sensors, and a `harmony_ir.send_raw` service. No HACS?
The hub even serves ready-to-paste `rest_command` YAML at `http://<hub-ip>/api/ha/rest_command.yaml`.

---

## 9. Security — do this before anything faces your LAN

- **Change the token from `harmonydev`.** It gates two dangerous surfaces:
  - the **dev shell** on `:2222` (`irapi devshell --token ...` — a plaintext root shell over TCP), and
  - the **OTA** endpoints (`/api/ota`, `/api/ota/db`).

  Edit [`../service/rcS.local`](../service/rcS.local) — change `--token harmonydev` on the `devshell`
  line — and change `OTA_TOKEN` in [`../service/irapi/src/web.rs`](../service/irapi/src/web.rs), then
  rebuild (§3) and redeploy. Pass your new token as the last argument to `tools/ota.py`.
- **Trusted LAN only.** The devshell is a developer convenience, **not** hardened SSH: single
  connection, token-in-cleartext, root shell. Never expose the hub (`:80`, `:2222`) to the internet or
  an untrusted network. There is no auth on the web UI itself.
- **Keep secrets out of git.** Real `wpa_supplicant.conf` (Wi-Fi creds) and any SSH keys are
  `.gitignore`d — keep them that way.
- **Revert anytime:** `rm /etc/init.d/rcS.local` (over devshell or UART) and reboot returns the hub to
  stock behavior. No flash partition was ever modified.
</content>
</invoke>

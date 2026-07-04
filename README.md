# Harmony Hub → Standalone IR Blaster

Turn a **Logitech Harmony Hub** into a fully **standalone, local network IR blaster and receiver**
— no Logitech account, no cloud, no phone app, no MyHarmony. The hub keeps running its original
Linux, but instead of Logitech's stack it runs a tiny self-contained appliance that exposes IR
over a clean HTTP API and a web UI, and drops straight into Home Assistant.

> **Status:** working on real hardware. Transmits and learns IR from Rust by driving the SoC's
> audio peripheral directly; ships a ~500-device offline code library, a native air-conditioner
> encoder, OTA updates, and a first-class Home Assistant integration.

<sub>Unofficial, not affiliated with or endorsed by Logitech. You do this to your own device at
your own risk. See [Safety](#safety).</sub>

---

## Why

The Harmony Hub is great hardware — three IR emitters, an IR receiver, Wi-Fi, a capable Atheros
SoC — chained to a cloud service that Logitech [shut down in 2024](https://support.logitech.com/).
This project keeps the hardware and throws away the dependency: everything runs **on the device,
on your LAN**, and is driven by open code you can read.

## What it does

- 📡 **Blast IR** from a browsable offline library of ~500 devices (TVs, AVRs, ACs, streamers…),
  built from the public-domain [Flipper-IRDB](https://github.com/Lucaslhm/Flipper-IRDB), plus
  on-device protocol encoders (NEC, Samsung, Sony SIRC, RC5, …) so codes are generated on the fly.
- 🎓 **Learn IR** — point any physical remote at the hub and capture its codes (stored on-device,
  browsable and re-sendable like any library code).
- ❄️ **Air conditioner control** — a native **Midea/Danby** climate encoder generates any
  temperature / mode / fan state from scratch (reverse-engineered and validated against real codes).
- 🌐 **HTTP API + web UI** on port 80 — browse, fire, learn, control the AC, set up Wi-Fi, update.
- 🏠 **Home Assistant integration** — remote + buttons + a climate thermostat + diagnostic sensors,
  with native `remote.learn_command`, all configured in the UI.
- ⬆️ **OTA updates** — flash new firmware or swap the code database over HTTP, no cables.

## How it works (the interesting part)

The hub runs an ancient Linux (2.6.31) on a big-endian MIPS **Atheros AR9331**, and its kernel is
built **without `CONFIG_FUTEX`**. That one detail shapes everything:

- **The runtime.** No futex means the **Go** runtime crashes at startup, so the appliance is
  **single-threaded Rust** — static, `musl`, `panic=abort`, **zero runtime crates**. (The
  dead-ends that proved this are in [`experiments/`](experiments/).)
- **The IR path.** Logitech's IR is buried in a closed `hal` daemon. Instead of talking to it, the
  appliance was reverse-engineered to drive the AR9331's **I2S audio peripheral** (`/dev/i2s`)
  directly: it renders a modulated carrier bitstream and DMAs it out the IR emitter GPIOs — and
  reads the demodulated receiver line back the same way to learn codes. Logitech's `hal` is not
  used at all. See [`docs/`](docs/) for the full teardown.

## Repository layout

```
service/
  irapi/               the appliance firmware — single-threaded Rust
    src/               i2s (IR driver) · midea (AC) · enc/db (codes) · learn · web (API+UI)
    codes/             bundled offline IR code database
    web/index.html     the self-contained web UI
  build-mips.sh        cross-compile recipe (big-endian MIPS, static musl)
  rcS.local            device boot script (starts the appliance, no Logitech stack)
  standalone-ir-bringup.sh
custom_components/     the Home Assistant integration (harmony_ir) — HACS-installable
homeassistant/         Home Assistant integration docs (setup + usage README)
tools/                 host tools: ota.py (HTTP update), flash backup, UART, RE helpers
ir-db/                 source IR captures
experiments/           early Go/Rust bring-up probes (why it's single-threaded Rust)
backups/               full-flash + per-partition dumps and backup tooling
docs/                  setup guides, HTTP API, CLI, and reverse-engineering notes
```

## Quick start

Run the guided installer — it selects your USB-serial adapter, installs the Rust
toolchain, builds the firmware, backs up the hub, and deploys our software:

```bash
git clone https://github.com/samuelleb11/logitechharmonyhub
cd logitechharmonyhub
./install.sh
```

`./install.sh` opens a menu; or drive it directly:

```bash
./install.sh deps        # install Rust 1.74 + the mips-unknown-linux-musl target
./install.sh build       # cross-build the firmware
./install.sh deploy      # push firmware over the network (hub already running our software)
./install.sh backup      # back up the hub flash over the serial console   [advanced]
./install.sh provision   # first-time install onto a fresh hub over serial  [advanced]
./install.sh all         # deps → build → deploy  (the usual update loop)
```

Then open the web UI at **http://\<hub-ip\>/**. First-time provisioning needs a
3.3 V USB-serial adapter on the **J2** pads (pad 2 RX, 4 TX, 8 GND) — the full
walkthrough is in **[docs/getting-started.md](docs/getting-started.md)**.

Reference docs: **[HTTP API](docs/http-api.md)** · **[CLI](docs/cli.md)** ·
**[Home Assistant](homeassistant/README.md)**.

## Home Assistant

Add the hub as a device and you get a `remote` (fire + learn/delete codes), per-function
`button`s, an optional AC `climate` thermostat, and diagnostic `sensor`s — all UI-configured,
with availability tracking. Install via **HACS** (add this repo as a custom *Integration*
repository) or copy [`custom_components/harmony_ir/`](custom_components/harmony_ir/) into your HA
`config/custom_components/`. See [homeassistant/README.md](homeassistant/README.md).

## Safety

- **Back up the flash first** ([docs/firmware-backup.md](docs/firmware-backup.md)) before changing
  anything.
- **Never erase or overwrite `mtd0` (U-Boot) or `mtd6`** — `mtd6` holds your unit's
  **irreplaceable per-device calibration** (radio cal, MAC). Bricking risk.
- All changes here are **overlay-only and reversible**; the stock firmware is left intact.
- The management API is **unauthenticated** and the built-in `devshell`/OTA token defaults to
  `harmonydev` — **change it**, and only run this on a **trusted LAN**.
- `backups/` contains raw flash images that include Logitech's copyrighted firmware; they are here
  only as personal recovery images for this specific unit.

## License

[GPL-3.0](LICENSE). This project builds on GPL'd platform components (Linux, madwifi). The
`*_decompiled_reference.lua` files under `service/irapi/` are decompiled excerpts of Logitech's
firmware, retained solely as reverse-engineering reference — they are Logitech's, not covered by
this project's license.

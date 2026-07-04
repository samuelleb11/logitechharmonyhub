# Documentation

Guides and reverse-engineering notes for turning a Logitech Harmony Hub into a standalone IR
appliance. Start with the [project README](../README.md) for the overview.

## Using it

| Doc | Contents |
|-----|----------|
| [getting-started.md](getting-started.md) | **⭐ Start here.** Build the firmware, get it onto the hub, connect Wi-Fi, use the web UI, update over the air, add to Home Assistant |
| [http-api.md](http-api.md) | The appliance's HTTP API reference (every endpoint, with `curl` examples) |
| [cli.md](cli.md) | The `irapi` command-line reference (appliance + developer/RE verbs) |
| [../homeassistant/README.md](../homeassistant/README.md) | The Home Assistant custom integration |

## Hardware & access (the reverse-engineering journey)

| Doc | Contents |
|-----|----------|
| [board-access-runbook.md](board-access-runbook.md) | **The end-to-end playbook:** powered-off board → untethered root shell over Wi-Fi (UART, U-Boot, uploads, Wi-Fi, devshell) + every gotcha |
| [hardware-specs.md](hardware-specs.md) | SoC, memory, flash layout, free space, userland tool inventory, peripherals, firmware versions |
| [uart-console.md](uart-console.md) | Physical UART access, baud (locked 115200), the `uartctl.py` tooling, catching U-Boot |
| [boot-and-uboot.md](boot-and-uboot.md) | U-Boot environment, boot flow, getting a readable verbose boot / root shell |
| [firmware-backup.md](firmware-backup.md) | How the full 16 MB flash was backed up (lua+RLE+base64 over UART) and verified |
| [usb-gadget-console.md](usb-gadget-console.md) | Using the USB port as a console/network gadget (no soldering) |

## Design & IR reverse-engineering

| Doc | Contents |
|-----|----------|
| [rf-remote-reverse-engineering.md](rf-remote-reverse-engineering.md) | The Harmony **remote** 2.4 GHz RF link (cc2544 radio, /dev/rfspi): protocol, pairing (Menu+Mute), and the button→IR plan |
| [ir-api-project.md](ir-api-project.md) | The project plan: exposing IR over a network API — architecture, language choice, recovery |
| [ir-service-buildplan.md](ir-service-buildplan.md) | The Rust appliance build plan and toolchain recipe |
| [ir-hardware-reverse-engineering.raw.txt](ir-hardware-reverse-engineering.raw.txt) | Research notes: how IR TX/RX maps onto the AR9331 I2S peripheral |
| [ir-protocol-reverse-engineering.raw.txt](ir-protocol-reverse-engineering.raw.txt) | Research notes: the IR carrier/bitstream/blob format |
| [ir-database-integration-plan.raw.txt](ir-database-integration-plan.raw.txt) | Research notes: building the offline code library |
| [ir-receive-learn-plan.raw.txt](ir-receive-learn-plan.raw.txt) | Research notes: the IR receive/learn path |
| [logs/](logs/) | Raw captured console logs (reference artifacts) |

## The device at a glance

- **Device:** Logitech Harmony Hub, codename **Pimento**.
- **SoC:** Atheros **AR9331 "Hornet"** (MIPS 24Kc, big-endian), board **AP121**. 32 MB RAM, 16 MB NOR flash.
- **OS:** Linux 2.6.31 + BusyBox 1.13.4 (Poky/Yocto, Feb 2020). Kernel has **no `CONFIG_FUTEX`**.
- **Console:** FTDI USB-TTL @ **115200 8N1**; U-Boot 1.1.4 (`bootdelay=0` — flood Enter to interrupt).
- **Backup:** full 16 MB flash captured & verified — see [firmware-backup.md](firmware-backup.md).

## How the appliance actually drives IR

The stock closed `hal` daemon owns the IR hardware, but the appliance **does not use it**. IR
transmit and receive were reverse-engineered down to the SoC's **I2S audio peripheral**
(`/dev/i2s`): a modulated carrier bitstream is DMA'd out the emitter GPIOs to transmit, and the
demodulated receiver line is oversampled and run-length-decoded to learn. The service is
**single-threaded static-musl Rust** (Go can't run — no futex; see [`../experiments/`](../experiments/)),
zero runtime crates, deployed native (UPX's stub `SIGTRAP`s on this kernel).

> Dates in these docs are absolute. Original RE gathered 2026-06-21.

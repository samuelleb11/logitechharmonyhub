# irapi — Harmony Hub IR appliance

Single-threaded, static `mips-unknown-linux-musl` HTTP/JSON IR service for the Logitech
Harmony Hub (standalone — no Logitech cloud). Drives the **AR9331 I2S peripheral directly**
(`/dev/i2s`) to transmit *and* receive IR — Logitech's `hal`/LTCP layer is **not** used (it was
removed). Full design:
[`../../docs/ir-service-buildplan.md`](../../docs/ir-service-buildplan.md).

**Hard constraint:** the device kernel has no `CONFIG_FUTEX`, so this stays **single-threaded**
— no `thread::spawn`, no async runtime, no contended locks. Zero runtime crates (hand-rolled
HTTP + JSON). Native deploy only (UPX's stub SIGTRAPs on this kernel).

## Build
```
bash ../build-mips.sh .          # -> target/mips-unknown-linux-musl/release/irapi  (~580 KB, MSB MIPS, static)
cargo build --release            # host build, for logic testing (encoders/JSON/DB)
```
(One-time toolchain setup: see the `rust-mips-toolchain` memory / `../build-mips.sh` header.)

## Status

> **Historical (Route A, superseded).** The milestone table and the "M1 plan" section below
> describe an earlier design that drove IR through Logitech's stock `hal` daemon over LTCP
> (`127.0.0.1:16716`). That approach was **abandoned** — `irapi` now drives `/dev/i2s`
> directly (TX and RX). The hal/LTCP reverse-engineering is retained for reference in
> [`../../docs/ir-service-buildplan.md`](../../docs/ir-service-buildplan.md).

| milestone | state |
|---|---|
| **M0** scaffold + toolchain (trait, LtcpBackend §2 framing, MockBackend, JSON, CLI) | ✅ builds for mips, logic host-verified |
| **M1** thin one-blast through `hal` (the proof) | next — see below |
| M2 HTTP server + `send_raw` + Mock | todo |
| M3 device DB + stored send | todo |
| M4 learn (`/ir/ir_cap`) | todo |
| M5 standalone boot + Wi-Fi + harden | todo |

## CLI
```
irapi serve  [--port 80]                                # the HTTP API + web UI (the appliance)
irapi fire   --device ID --function NAME [--select 7]   # fire a code from the offline library
irapi ac     [--power on|off] [--mode cool] [--fan auto] [--temp 22]   # Midea/Danby AC encoder
irapi learn  [--secs 15] [--device ID --function NAME]  # capture a remote button (direct I2S RX)
irapi codes  [--device ID]                              # list library devices / functions
```
Direct-hardware dev/RE tools also exist: `i2sraw`, `i2stest`, `i2scap`, `i2sgpiosweep`,
`peek`/`poke`/`regs` (`/dev/mem`), `devshell`, `dhcpd` — run `irapi --help` for the full list.
(The old `blast`/`health`/`mock` verbs and the LTCP `--raw-hex`/`--dry-run` flags were removed
with the hal backend; `learn` is now the direct-I2S capture verb.)

## M1 plan + the blob dependency (read before the next device session)
M1 proves the LTCP↔`hal` seam with **one real IR blast**. But `ir_send` needs an actual IR
**blob**, and the only ways to get one are:
1. **Learn it** (`/ir/ir_cap`, = M4) — point the TV's real remote at the hub, capture. This
   needs no cloud config, so **the realistic first test is the learn→replay loop**: arm capture →
   press the TV remote → get a blob → replay via `ir_send` → TV reacts. This retires the `ir_send`
   AND `ir_cap` framing together (build-plan §2.6 #1/#2/#4) and confirms `hal` runs standalone (R1).
2. **A cached blob from the backup:** the stock `data` partition (`backups/mtd4.bin`, jffs2) may
   hold IR blobs from a prior configuration — worth grepping as a no-remote blob source.
3. Capturing a real stock `/ir/ir_send` is impractical here: the stock app only emits IR for
   cloud-provisioned activities (the cloud is dead), and the device has no `tcpdump`.

So the next session: boot → standalone bring-up (`../standalone-ir-bringup.sh`: cc2544 + firmware +
`hal`) → verify `hal` binds `:16716` → implement minimal `ir_cap` → teach one TV button → replay.

## Layout
```
src/main.rs   CLI
src/i2s.rs    direct AR9331 I2S IR driver (TX render/blast + RX capture/RLE)
src/enc.rs    on-device IR protocol encoders (NEC/Samsung/SIRC/RC5/...)
src/midea.rs  native Midea/Danby AC climate encoder
src/db.rs     offline IR code library (/cache/irdb.txt) + learned codes (/cache/custom.json)
src/learn.rs  capture a physical remote, decode to mark/space
src/web.rs    HTTP API + self-contained web UI (the `serve` command)
src/json.rs   hand-rolled JSON Value + parser + serializer
src/mem.rs    /dev/mem MMIO peek/poke helpers
```

# irapi — Harmony Hub IR appliance

Single-threaded, static `mips-unknown-linux-musl` HTTP/JSON IR service for the Logitech
Harmony Hub (standalone — no Logitech cloud). Talks **LTCP to the stock `hal` daemon**
(`127.0.0.1:16716`) which drives the `cc2544` IR hardware. Full design:
[`../../docs/ir-service-buildplan.md`](../../docs/ir-service-buildplan.md).

**Hard constraint:** the device kernel has no `CONFIG_FUTEX`, so this stays **single-threaded**
— no `thread::spawn`, no async runtime, no contended locks. Zero runtime crates (hand-rolled
HTTP + JSON). Native deploy only (UPX's stub SIGTRAPs on this kernel).

## Build
```
bash ../build-mips.sh .          # -> target/mips-unknown-linux-musl/release/irapi  (~580 KB, MSB MIPS, static)
cargo build --release            # host build, for logic testing / MockBackend
```
(One-time toolchain setup: see the `rust-mips-toolchain` memory / `../build-mips.sh` header.)

## Status — M0 done
| milestone | state |
|---|---|
| **M0** scaffold + toolchain (trait, LtcpBackend §2 framing, MockBackend, JSON, CLI) | ✅ builds for mips, logic host-verified |
| **M1** thin one-blast through `hal` (the proof) | next — see below |
| M2 HTTP server + `send_raw` + Mock | todo |
| M3 device DB + stored send | todo |
| M4 learn (`/ir/ir_cap`) | todo |
| M5 standalone boot + Wi-Fi + harden | todo |

## CLI (M0)
```
irapi blast --blob-hex <hex> | --blob-b64 <b64> [--ports 0,1,2] [--repeat 1] [--dry-run]
irapi blast --raw-hex <hex>          # replay exact captured LTCP bytes (M1)
irapi learn [--timeout 15]           # (M4 stub)
irapi health                         # is hal reachable on :16716?
irapi mock                           # exercise the stack with no device
```
`--dry-run` prints the provisional LTCP frame so it can be diffed against real captured bytes.

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
src/ir.rs     IrBackend trait, IrCode, LtcpBackend (§2 framing), MockBackend
src/json.rs   hand-rolled JSON Value + parser + serializer
src/util.rs   hex / base64
```

# IR Service — Consolidated Build Plan

> **SUPERSEDED (Route A / hal).** This plan builds `irapi` as an HTTP front end that drives IR
> through Logitech's stock `/usr/bin/hal` daemon over LTCP (`127.0.0.1:16716`) — "Route A". That
> approach was **later abandoned**: the shipping `irapi` drives the AR9331 I2S peripheral
> (`/dev/i2s`) **directly** for both TX and RX, with on-device protocol encoders + a native
> Midea/Danby AC encoder, and serves the API + web UI on a **single port :80**. There is no `hal`,
> no LTCP, and no `LtcpBackend`/`MockBackend` in the current code. The §2 LTCP/HBus spec and the
> Route-A architecture below are kept as **reverse-engineering history**, not the current design.

> **PROGRESS (2026-06-21):** **M0 done** — `service/irapi/` scaffolded: builds to a 580 KB
> static MSB-MIPS binary, logic host-verified (IrBackend trait, LtcpBackend §2 framing,
> MockBackend, hand-rolled JSON, `blast`/`learn`/`health`/`mock` CLI with `--dry-run`/`--raw-hex`).
> **M1 refinement:** `ir_send` needs a real IR *blob*, and the only no-cloud way to get one is to
> *learn* it — so the realistic first device test is the **learn→replay loop** (arm `/ir/ir_cap` →
> press the TV remote → replay via `/ir/ir_send` → TV reacts), which retires the `ir_send` AND
> `ir_cap` framing together. Alt blob source: grep `backups/mtd4.bin`. See `service/irapi/README.md`.
>
> **PROGRESS (2026-07-02) — live device session:** **R1 RETIRED** — `hal` runs STANDALONE and
> listens on `127.0.0.1:16716`. Verified bring-up = `ifconfig lo up` (CRITICAL — hal binds loopback;
> the workflow script was missing it) + dbus + `insmod cc2544.ko` + firmware + `hal -f -s`
> (`service/standalone-ir-bringup.sh` fixed). **`irapi` (Rust) connects to `hal` over LTCP**
> (`health` → connected). **Still open:** our provisional single-packet frame is 108 B and `hal`
> resets it — need the 64-byte primary+secondary packetization (§2.6 #2); reverse `ltcp.lua`
> `formLtcpPacket` + iterate against live `hal` (which doesn't log parse errors). `hal` left running.
> Gotcha: under `init=/bin/sh`, `/tmp` is the persistent jffs2 overlay (no tmpfs) — it fills up.

**Status:** build-ready. Consolidates four parallel investigations (LTCP/HAL protocol RE,
standalone HAL bring-up RE, Wi-Fi bring-up RE, Rust service design) into one decisive plan.
**Lead-engineer call:** Build **Route A** (HTTP/JSON Rust front end → LTCP → stock `/usr/bin/hal`
→ `cc2544`). Treat the IR `code` as an **opaque blob** (learn via `/ir/ir_cap`, replay verbatim
via `/ir/ir_send`) — do **not** reimplement Logitech's `irgen`/`irdecoder` codec. Single-threaded
Rust, static `mips-unknown-linux-musl`, native deploy, overlay-only (no flashing).

> **Canonical storage path = `/mnt/data`** (the real jffs2 overlay mount, ~4 MB free; `/cache`
> ~5 MB secondary). The older `docs/ir-api-project.md` says `/data` — that is imprecise; use
> `/mnt/data` everywhere. Verified in `docs/hardware-specs.md`.

---

## 1. Architecture

```
                    LAN (Wi-Fi: ath0, WPA2-PSK, DHCP)
                                  │  HTTP/1.1 + JSON  (single port, default 0.0.0.0:80)
                                  ▼
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  irapi  — single OS thread, static mips-musl, ~0.8 MB, on /mnt/data        │
  │                                                                            │
  │   http (hand-rolled) ── router ── api handlers                            │
  │        │                              │                                    │
  │        │                       DeviceDb (JSON on /mnt/data, atomic save)   │
  │        │                              │                                    │
  │        └──────────────► IrBackend (trait)  ◄── the swap seam              │
  │                            ├── LtcpBackend  ── LTCP 127.0.0.1:16716 ──┐    │
  │                            ├── MockBackend  (host dev / tests)        │    │
  │                            └── RawSpiBackend (Route B, future)        │    │
  └──────────────────────────────────────────────────────────────────────┼────┘
                                                                          ▼
                              /usr/bin/hal  (stock, unmodified)   ── HBus /ir/ir_send, /ir/ir_cap
                                  │  open/read/write (NO ioctl), length-prefixed frames
                                  ▼
              /dev/rfspi (10,59)  +  /dev/rffw (10,60)   ←  cc2544.ko
                                  ▼
                       TI CC2544 (8051 RF SoC) → built-in IR blaster + 2 mini-jack extenders
```

**Standalone boot stack** (no luaworks, no cloud; one line added to the overlay `rcS.local`):

```
power → U-Boot → kernel 2.6.31 → stock rcS → rcS.local
                                                 └─► /mnt/data/irapi/boot/irapi-bringup.sh &
        ┌────────────────────────────────────────────────────────────────────────┐
        │ 1. volatile dirs + vm.overcommit + syslog                               │
        │ 2. insmod cc2544.ko ; cat cc2544.bin > /dev/rffw   (IR ready)           │
        │ 3. dbus-daemon --system           (hal calls dbus_bus_get for BT only)  │
        │ 4. seed /var/watchdog/hal ; start /usr/bin/watchdog                     │
        │ 5. /usr/bin/hal -s &              (LTCP on 127.0.0.1:16716)             │
        │ 6. net-up.sh: insmod ath stack → wlanconfig ath0 sta → wpa_supplicant   │
        │              → udhcpc                                                    │
        │ 7. /mnt/data/irapi/irapi --config … &                                   │
        └────────────────────────────────────────────────────────────────────────┘
```

**Paragraph.** A LAN client makes an HTTP/JSON request. `irapi` (one OS thread; no
`thread::spawn`, no async runtime, no contended locks — so the `CONFIG_FUTEX=n` kernel is never
hit) parses it with a hand-rolled HTTP/JSON stack, resolves the request against the on-overlay
device DB, and dispatches through the `IrBackend` trait. The production `LtcpBackend` opens a TCP
socket to the **stock `hal` daemon** on `127.0.0.1:16716`, frames an LTCP `command` (send) or
`open/read/close` (learn) packet, and `hal` drives the `cc2544` over GPIO-SPI to emit/capture IR.
We reuse Logitech's entire carrier/duty/codec path inside `hal`; we never open `/dev/rfspi`
ourselves (Route A). The hub runs standalone because `hal` is an independent C daemon — luaworks
is only its *client*. We start `hal` ourselves and keep the hardware watchdog fed via `hal`'s own
`/var/watchdog/hal` heartbeat. `hal` is multithreaded but links uClibc **LinuxThreads** (clone +
SysV sems + SIGUSR1/2, zero futex symbols), which is exactly why the stock firmware runs on a
futex-less kernel — the futex constraint binds only *our* runtime, hence single-threaded Rust.

---

## 2. LTCP + ir.send + ir.cap spec

> Source of truth: bytecode-exact parse of `opt/luaworks/tasks/hal/core/{ltcp,hbus}.lua`,
> `hal/main.lua`, `harmonyengine/core/{irsender,irmanager}.lua`, plus jansson format-string scan +
> capstone disasm of `usr/bin/hal`. Confidence is rated per item; **load-bearing unknowns and the
> exact step to retire each are in §2.6.**

### 2.1 Connection  — **HIGH confidence**
- TCP connect `127.0.0.1:16716` (0x414C). Set **TCP_NODELAY**.
- Do **not** use 16717 (0x414D) — that is the RF/HLAPI dongle path, not the LTCP listener.
- Raw LTCP frames back-to-back; **no extra TCP-level length prefix**. Read available bytes, feed a
  reassembly state machine, frame purely from LTCP headers below.
- Set `SO_RCVTIMEO`/`SO_SNDTIMEO` on this socket so a wedged `hal` can't hang our single thread.

### 2.2 Wire framing — **HIGH confidence (single-packet); MEDIUM (multi-packet)**

Constants (bytecode-exact):
```
SERVICE_ID = 0xFF   (0xFE = error packet)      LTCP_PACKET_SIZE = 64
RESP_HDR_SIZE = 4   SEC_HDR_SIZE = 2           CHECKSUM_SIZE = 1 (XOR, appended)
```

**Primary request packet** = `formPrimaryPacket(command, requestId, args[])`:
```
[0]=0xFF (SERVICE_ID)
[1]=command opcode               (IR send uses command=8; cap uses open=1/read/close=7)
[2]=msgId                        (1-byte auto-increment seq; wraps; correlate replies by this)
[3]=numParam | 0x80              (low 6 bits = #params; bit7 set by sender)
[4..]=per-param TLV (see below)
[last]=XOR checksum over the WHOLE buffer (bytes [0..len-1])
```
Opcodes (recovered): `open=1 seek=2 write=3 flush=5 devctl=6 close=7 command=8 notify=9`
(`read`/`ping` fall in the gaps 4/10 — not needed for IR).

**Per-param TLV** `formParameter(arg)`:
- string/binary: `[0x80] [len_low_byte] [len bytes …]`  — the IR JSON and the IR `code` blob both
  use this.
- number: 0x80-tagged byte-wise form (not needed — IR numerics ride inside the JSON).

**XOR checksum** (exact):
```rust
let mut crc = 0u8;
for &b in &buf[0..len] { crc ^= b; }   // single byte, appended at the end
```

**Multi-byte integers = big-endian (MSB first)** on read; writer matches.
(Exception: the 2-byte *secondary*-packet length field is written `lsb`, then `msb|flags`.)

**Secondary (continuation) packets** — only if payload won't fit one primary packet:
`numPackets = ceil(len / (64 - 2 - 1)) = ceil(len/61)`; each 64-byte packet is
`[2-byte sec header] [≤61 data] [1 XOR]`, header `lsb = len&0xFF`, `msb = (len>>8)&0x3F`, with
bits `0x80`/`0x40` as continuation/flag. **An IR `code` blob can exceed 61 bytes, so multi-packet
send must work** — this is unknown #2 in §2.6.

**Response primary header** `processPrimaryPacket`:
```
[0]=serviceId  (0xFF ok; 0xFE = LTCP error packet; an EOF/NoMoreData marker also exists)
[1]=command    [2]=reqId (echoes your msgId)    [3]=numParam (&0x3F = count; 0x80 = isResp)
then params (TLV), then 1 XOR byte.
```
LTCP error enum: `General, SequenceMismatch, Busy, BadVersion, UnknownHandle, UnknownAction,
AlreadyAborted, NoMoreData, InvalidAddress, InvalidCommand, BadDataLength, BadRegion,
CheckSumMismatch, TooManyFileOpen`.

### 2.3 ir.send — **HIGH confidence**
HBus path **`/ir/ir_send`** (built-in blaster + 2 extenders). Ignore `/rf/hot/1/ir/ir_send`
(paired-RF-dongle only). The hbus `command` (opcode 8) carries **two LTCP params**:

- **param #1 = JSON envelope** (UTF-8):
  ```json
  {"id":42,"cmd":"/ir/ir_send","data":{"enable":true,"keyLatency":500,"ports":1},"timeout":3000}
  ```
- **param #2 = binary IR `code` blob** (the captured/compiled waveform).

`data` fields (hal unpack signature `{s:b, s?i, s?i}` = `enable` + two optional ints):
- `enable` (bool, required): `true` = fire; `false` = stop/cancel (`cancelIr`).
- `keyLatency` (int ms): inter-repeat gap / hold cadence. Stock heartbeat 500/503 ms.
- `ports` (int bitmask): which emitters fire. **Opaque int** — bit→emitter mapping is unknown #1
  (§2.6). **Carrier freq & duty are NOT JSON fields — they are encoded inside the `code` blob**
  (hal derives `period_ns`/`duty_cycle`/`pulse_bclk`/`min_repeat` from it). Freq/duty appear as
  explicit args only on the diagnostic `/ir/ir_loopback` + `/ir/ir_test` paths.

**Hold/repeat:** keep firing by re-sending the heartbeat (`enable=true`, same `keyLatency/ports`)
every ~`keyLatency` ms while held; `hal` does per-frame auto-repeat. **Cancel:** send
`enable=false` on the same path. **Status:** app-level JSON reply carries HTTP-style `code`:
`200 "OK"`; `500 "RF timed out"`/`503 "RF link lost"` are RF-dongle-only (expect 200 for built-in IR).

### 2.4 ir.cap (learn) — **MEDIUM confidence**
HBus path **`/ir/ir_cap`**, modeled as **open → read(raw) → close** over the LTCP file ops
(`open=1`, `read`, `close=7`) — *not* the simple `command=8` path. hbus.lua special-cases `ir.cap`
to hold the connection open for the async result.
1. `open` on `/ir/ir_cap` → hal opens the cc2544 receiver (`libhal_ir_cap_open`).
2. user points remote + presses → hal captures edges (`libhal_ir_cap_raw_read`).
3. `read` response delivers the **raw captured blob** (hal works in nanoseconds; carrier/duty are
   embedded in the blob, same form `/ir/ir_send` consumes).
4. timeout/abort → `close`(7) on `/ir/ir_cap` (hal logs "Timeout occurred - no heartbeat or cancel").

**Store the returned blob verbatim; replay it verbatim as param #2 of `/ir/ir_send`.** This is the
whole point of the opaque-blob strategy — it sidesteps the `irgen`/`irdecoder` codec entirely.

### 2.5 Reply / error — **HIGH (transport) / MEDIUM (app)**
Transport: response header + TLV params + XOR (correlate by `reqId`); `0xFE`=error packet.
App: response param is a JSON object with an HTTP-style `code` (200 OK) and an `error_code` field
on failure (hal `create_json_resp`/`hot_set_ack_error_code`); bad JSON → `w_hbus_response_json_decode_error`.

### 2.6 Load-bearing unknowns + exact retirement step

> **Constraint:** a live loopback sniff requires the stock app running, which contradicts
> standalone. Prefer **static Ghidra on `hal`/`luaworks.so`**, or a **one-time controlled boot of
> the stock app** to capture `127.0.0.1:16716` loopback (e.g. tcpdump/strace on the device, or a
> shim listener) — then return to the standalone stack. Most items below are retired fastest by
> **M1's live one-blast test**, not by more static work.

| # | Unknown | Impact | Retire by |
|---|---------|--------|-----------|
| 1 | `ports` bitmask bit→emitter map (built-in vs jack1/jack2; is "all"=0x07?) | which emitter fires | **M1 live test:** fire with `1,2,4,7`, watch each emitter via phone camera. Cheapest first. |
| 2 | `formParameter` length encoding for >255 B + secondary-packet 2-byte header flag semantics | multi-packet sends of large IR blobs | **Ghidra** on `formLtcpPacket`/`formParameter`, OR capture one real stock send of a long code. Implement single-packet first; gate multi-packet behind this. |
| 3 | Exact IR `code` blob binary layout | only if we ever generate codes | **Sidestepped** — learn+replay opaque blob (recommended). Reverse `luaopen_irdecoder` in Ghidra only if codegen is later wanted. |
| 4 | `ir_cap` result framing (single vs streamed read, end-marker, separate carrier field) | learn correctness | **M2/M4 live capture** against `hal`; instrument the read loop. |
| 5 | numeric `read`/`ping` opcodes (gaps 4/10) | only if file sub-proto used | confirm in Ghidra when implementing `ir_cap` open/read/close. |

**Decisive call:** retire #1 and #2-single-packet in **M1** (one live blast). Do **not** spend more
static effort on #3 — opaque-blob replay makes it irrelevant for v1.

---

## 3. Standalone bring-up (reversible, recovery-safe)

Two scripts on the overlay. `irapi-bringup.sh` is the master; `net-up.sh` is the Wi-Fi stage
(separated so failures are isolated and visible). **Nothing here writes any mtd partition.**
Existing reference scripts: `service/standalone-ir-bringup.sh` (HAL/IR/watchdog stages, verified)
— fold it into the layout below.

### 3.1 `/mnt/data/irapi/boot/irapi-bringup.sh` (master)
Stages, each logged and non-fatal (so a failure is debuggable, never bricking):
1. **volatile dirs + tuning:** `mkdir -p /var/volatile/{cache,lock,log,run,tmp,watchdog,dbus}`;
   ensure `/var/watchdog` resolves; `echo 1 > /proc/sys/vm/overcommit_memory`; start `syslogd`/`klogd`.
2. **IR stack (idempotent):** if `/dev/rfspi` absent or `cc2544` not in `/proc/modules`:
   `insmod /lib/modules/$(uname -r)/cc2544.ko` then `cat /lib/firmware/cc2544.bin > /dev/rffw`.
   (hal re-checks `/lib/firmware/cc2544.version` and only reflashes on mismatch — pre-load is
   harmless, not a loop. cc2544 fw lives in volatile chip RAM → reloaded each cold boot, ~21 KB, expected.)
3. **dbus system bus:** `dbus-uuidgen --ensure`; `dbus-daemon --system`. hal calls `dbus_bus_get()`
   at startup (for `org.bluez` only); IR never touches dbus, but bringing the bus up de-risks a
   possible abort and matches stock ordering.
4. **watchdog:** seed `: > /var/watchdog/hal`, then start `/usr/bin/watchdog` (15 s HW timeout,
   scans `/var/watchdog/*` every 7 s, stops kicking if any file's mtime is >15 s stale). Guard with
   `[ ! -e /etc/nowatchdog ]` for a kill switch during bring-up.
5. **hal:** `/usr/bin/hal -s &`  — **never pass `-w`** while `watchdog` runs (that disables hal's
   `/var/watchdog/hal` heartbeat thread → guaranteed reboot loop). Keep `LD_PRELOAD=/lib/libcrashlog.so`
   optional (drop if it segfaults under the minimal env). Wait for `00000000:414C` in `/proc/net/tcp`.
6. **network:** `[ -x …/net-up.sh ] && …/net-up.sh` (stage 3.2).
7. **service:** `/mnt/data/irapi/irapi --config /mnt/data/irapi/config.json &` (optionally wrapped
   in a `while :; do irapi …; sleep 2; done` respawn loop — still single-threaded).

**Graceful degradation:** if hal won't bind, still start `irapi` (health reports `hal:"down"`) so
the box is reachable for debugging rather than dark.

### 3.2 `/mnt/data/irapi/boot/net-up.sh` (Wi-Fi, WPA2-PSK station) — **verified**
```sh
LM=/lib/modules/$(uname -r)
# 1. Atheros umac stack in dependency order (creates wifi0; cal data auto-read from flash by
#    ath_hal). Do NOT load art.ko (TDE/manufacturing only).
for m in asf adf ath_hal ath_rate_atheros ath_dev umac; do
  [ -d /sys/module/$m ] || insmod $LM/$m.ko
done
# wait for base radio wifi0
i=0; while [ ! -d /sys/class/net/wifi0 ] && [ $i -lt 50 ]; do usleep 100000; i=$((i+1)); done
# 2. station VAP ath0 (idempotent)
[ -d /sys/class/net/ath0 ] || /sbin/wlanconfig ath0 create wlandev wifi0 wlanmode sta
/sbin/ifconfig ath0 up
# 3. associate — driver backend = wext (this wpa_supplicant 0.7.2 has only wext+madwifi; NO nl80211)
mkdir -p /var/run/wpa_supplicant
/usr/sbin/wpa_cli -i ath0 ping >/dev/null 2>&1 || \
  /usr/sbin/wpa_supplicant -s -B -Dwext -iath0 -c /etc/wifi/wpa_supplicant.conf
# wait for wpa_state=COMPLETED (poll wpa_cli status, ~30 s)
# 4. DHCP (busybox udhcpc; stock ath0 opts)
/sbin/udhcpc -i ath0 -b -S -t 5 -T 2 -A 20 -p /var/run/udhcpc.ath0.pid \
  -s /usr/share/udhcpc/default.script
```
- **Interface naming is order-sensitive and three-tiered:** `wifi0` (radio, made by umac insmod) →
  `ath0` (station VAP). Never `wlan0`. Run wpa_supplicant/udhcpc/iwconfig against `ath0`.
- **Use `wext`, not nl80211** (absent in this build); `madwifi` is the only fallback.
- **Cal/regdomain auto-read by `ath_hal` from mtd6** at insmod — no userspace cal/`iw reg` step; do
  **not** overwrite mtd6.
- **Credentials:** `/etc/wifi/wpa_supplicant.conf` (overlay, `chmod 600`, `key_mgmt=WPA-PSK`,
  `proto=RSN WPA`, `pairwise=CCMP TKIP`). Ship `wpa_supplicant.conf.example` in the repo with
  placeholders; the real file is generated on-device — **never commit creds.**
- **Verify up:** `wpa_cli -i ath0 status` → `wpa_state=COMPLETED` **and** `ath0` has a non-zero
  inet addr; `default.script` writes `/etc/resolv.conf` (overlay-writable).

### 3.3 Boot hook (reversible)
Append to overlay `/etc/init.d/rcS.local` (create if absent — it's an overlay file, shadows
squashfs, no flash):
```sh
# --- IR appliance (added by irapi install; delete this block to disable) ---
[ -x /mnt/data/irapi/boot/irapi-bringup.sh ] && /mnt/data/irapi/boot/irapi-bringup.sh &
# --- end IR appliance ---
```
**Reversibility/recovery:** everything is overlay files. Disable = delete the block (or
`rm /mnt/data/irapi/irapi`); uninstall = `rm -rf /mnt/data/irapi` + remove block. If `rcS.local`
gets mangled, deleting the overlay copy reverts to the squashfs stock. U-Boot serial recovery of
mtd1–mtd5 from `backups/*.bin` remains available; **never touch mtd0 (u-boot) or mtd6 (cal)**.

---

## 4. Rust crate, dependencies, REST API + DB

### 4.1 Crate layout — single binary `irapi`
```
service/irapi/
  Cargo.toml            # profile cloned from rustprobe: opt-level=z, lto, codegen-units=1,
                        #   panic=abort, strip   (verified: ~547 KB native probe)
  .cargo/config.toml    # cloned verbatim from experiments/rustprobe/.cargo/config.toml (rust-lld,
                        #   static, -no-pie, /tmp/mipslibs)
  build-mips.sh         # reuse service/build-mips.sh
  src/
    main.rs             config.rs
    http/{mod,request,response,router}.rs   # hand-rolled HTTP/1.1 subset
    json/mod.rs                              # hand-rolled Value enum + parser + serializer
    api/{mod,send,learn,devices,sys}.rs
    ir/{mod,backend,ltcp,mock}.rs            # IrBackend trait + LtcpBackend + MockBackend
    db/{mod,store}.rs                        # DeviceDb + atomic temp+fsync+rename
    util.rs                                  # hex/base64/error/time
```

### 4.2 Dependency decisions — **zero runtime crates**
- **HTTP:** hand-rolled, blocking, **one connection serviced to completion before the next
  `accept()`**. Justified: tiny bursty traffic; the single IR emitter + single `hal` socket force
  serialization anyway; one thread = no `thread::spawn`, no contended `Mutex`, no `Arc` → **futex
  never reached**; smallest binary. Parse request line + headers + `Content-Length` body only (no
  chunked, no TLS). Cap headers (8 KB) and body (256 KB). Support keep-alive; `Connection: close`
  on any parse error. The per-conn handler catches all errors → HTTP response; **the accept loop
  must never die.** Set read timeouts on client sockets.
- **JSON:** **hand-rolled, no serde.** `serde_json` adds ~150–300 KB of monomorphized code under
  `opt-level=z` and fights LTO; the schema is a dozen structs fully under our control. One
  `enum Value { Null, Bool, Num(f64), Str, Arr, Obj(Vec<(String,Value)>) }` (Vec-of-pairs keeps
  key order stable for a human-diffable on-disk DB; no hasher) + recursive-descent parser +
  serializer (~300 lines). Integers (µs timings, masks) emitted as ints. *Only* fallback if it
  proves tedious: `nanoserde` (measure size first). **Avoid** `serde_json`/`miniserde`/`tokio`/`mio`.
- **Everything else:** `std` only (sockets, `SystemTime`, files). Logging = 3-line stderr macro;
  init redirects stderr to `/mnt/data/irapi/irapi.log`. **No `log` crate.**

### 4.3 The seam — `IrBackend` trait
```rust
trait IrBackend {
    fn send(&mut self, code: &IrCode, ports: u8, repeat: u16) -> Result<(), IrError>;
    fn learn_start(&mut self, port: Option<u8>, timeout_s: u16) -> Result<SessionId, IrError>;
    fn learn_poll(&mut self) -> Result<LearnState, IrError>; // Pending | Captured(IrCode) | Expired
    fn learn_stop(&mut self) -> Result<(), IrError>;
    fn health(&mut self) -> BackendHealth;                   // hal connected? listener up?
}
```
`LtcpBackend` owns all §2 framing (XOR, SERVICE_ID, TLV, opcode 8 for send, open/read/close for
cap) and the opaque-blob passthrough. `MockBackend` runs the whole stack on the host. `RawSpiBackend`
(Route B) can drop in later behind the same trait.

### 4.4 IR code model + DB schema
**`ir::IrCode`** (encoding-neutral unit the backend consumes):
- `encoding`: `"ltcp_blob"` (opaque captured bytes — **the v1 path**, base64 in JSON), with room
  for `"raw"` (µs pairs) / `"pronto"` later if codegen is added.
- `carrier_hz` (u32, informational — actually embedded in the blob), `duty` (u8 %, optional),
  `ports` (`Vec<u8>` port ids → bitmask in `ltcp.rs`), `repeat` (u16).

**On-disk DB `/mnt/data/irapi/db.json`:**
```json
{ "schema":1,
  "devices":[
    { "name":"tv-samsung", "type":"tv", "manufacturer":"Samsung", "model":"UE55",
      "defaults":{"carrier_hz":38000,"duty":33,"ports":[0,1,2],"repeat":1},
      "buttons":[
        { "key":"power","label":"Power",
          "code":{"encoding":"ltcp_blob","carrier_hz":38000,"duty":33,
                  "ports":[0],"repeat":1,"data":{"blob_b64":"…"}} } ] } ] }
```
- **Default emitters = ALL** (built-in + both jacks; `ports:[0,1,2]`) per the user requirement,
  pending the §2.6-#1 bit-map confirmation.
- **Resolution order at send:** request body → button code → device defaults → fallback (all ports,
  repeat 1).
- **Atomic save on jffs2:** write `db.json.tmp` → `f.sync_all()` → `fs::rename` (atomic) →
  fsync dir. On write error, **roll back the in-RAM mutation and return 500** (RAM never diverges
  from disk). On startup: missing/corrupt `db.json` → try `db.json.tmp` → else start empty + warn,
  never crash.
- **Size guardrails:** ~400–600 B/button JSON; 50-button device ≈ 30 KB; 100 devices ≈ 3 MB —
  fits the ~4 MB overlay but enforce a configurable `max_db_bytes` (default 1 MB), reject `PUT`
  that would exceed it, surface `db_bytes` in `/api/health`. One canonical encoding per button.

### 4.5 REST API (base `/api`, JSON; single port, default bind `0.0.0.0:80` — API + web UI together)
Uniform error envelope `{"ok":false,"error":"…","detail":"…"}`. Home-Assistant-friendly.

```
GET    /api/health                                   -> version, uptime, backend, hal status, db_bytes
GET    /api/ports                                     -> [{id,name,kind}] for blaster + 2 jacks
POST   /api/devices/{d}/buttons/{b}/send              -> fire stored code  {ports?,repeat?,port_mask?}
POST   /api/send_raw                                  -> fire ad-hoc code  {encoding,ports?,repeat?,data}
POST   /api/learn/start                               -> {session,state:"armed",expires_in_s} (409 if busy)
GET    /api/learn/result                              -> 202 pending | 200 {code} | 410 expired (POLLED)
POST   /api/learn/stop                                -> {state:"idle"}
GET    /api/devices                                   -> list (name,type,button count)
POST   /api/devices                                   -> create (409 dup); PATCH/DELETE /api/devices/{d}
GET    /api/devices/{d}                               -> full device incl. buttons
GET    /api/devices/{d}/buttons                       -> list
PUT    /api/devices/{d}/buttons/{b}                   -> create/replace button code (200/201)
DELETE /api/devices/{d}/buttons/{b}
```
- `{d}`/`{b}` are slugs `[A-Za-z0-9._-]{1,64}`; display names go in `label`/`manufacturer`.
- **LEARN is non-blocking/polled** — `start` arms capture and returns immediately; `result` does
  one short timeout-bounded `hal` read and returns 202 or 200. The single accept loop never blocks
  more than one short socket timeout. Captured code shape == stored code shape (PUT it straight in).
- Status codes: 200/201/202(pending)/400/404/409/410(learn expired)/500(DB write)/502(hal error)/503(backend down).
- Mutating calls atomic-save before returning success.

---

## 5. Milestones

> Each milestone names the **riskiest unknown to retire first**. Order is chosen so the highest-risk
> seam (LTCP↔hal) is proven before anything is built on it.

**M0 — Toolchain skeleton (host, ~0.5 d).** Clone `rustprobe` Cargo/`.cargo/config.toml` into
`service/irapi`; build empty `main()` via `build-mips.sh`; confirm `file` = MSB MIPS static, sane
size. *Deliverable:* compiling crate skeleton. *Retire:* nothing new (toolchain already verified).

**M1 — Thin one-blast (THE PROOF).** Hardcode one captured/known IR blob; `LtcpBackend::send`
opens `127.0.0.1:16716`, frames one `/ir/ir_send` (opcode 8, JSON param + blob param + XOR), fires.
CLI entry `irapi blast`, no HTTP yet. *Deliverable:* **one real IR blast through stock `hal`, a TV
reacts.** *Retire first:* **§2.6-#1 (`ports` bitmask: try 1/2/4/7, watch each emitter via phone
camera) and §2.6-#2 single-packet framing** — the load-bearing LTCP↔hal seam. Also confirms
standalone `hal` actually runs without luaworks (drives M5 fallback).

**M2 — HTTP + send_raw + Mock.** Hand-rolled HTTP/1.1 + JSON; `/api/health`, `/api/ports`,
`/api/send_raw`; `MockBackend` for host iteration. *Deliverable:* blast over the network.
*Retire:* HTTP/JSON binary-size budget under `opt-level=z` (measure; fall back to `nanoserde` only
if needed) and the no-futex single-thread server pattern at real request volume.

**M3 — Device DB + stored send.** `DeviceDb` atomic save/load on `/mnt/data`; CRUD; stored-button
send with defaults/override resolution; size guardrails + `db_bytes`. *Deliverable:* persistent
device/code DB driving real sends. *Retire:* jffs2 atomic-write durability across power loss
(temp+fsync+rename; test by yanking power mid-write) and the overlay size budget.

**M4 — Learn (capture).** `LtcpBackend` `/ir/ir_cap` open/read/close; non-blocking
`/api/learn/{start,result,stop}`; captured blob PUT-able into a button. *Deliverable:* full
send+learn+DB appliance — teach a TV from its real remote, replay it. *Retire first:* **§2.6-#4
(ir_cap result framing: single vs streamed read, end-marker)** — confirm by live capture against `hal`.

**M5 — Standalone boot + harden.** `irapi-bringup.sh` + `net-up.sh` + `rcS.local` hook; graceful
hal-down degradation; stderr→logfile; respawn loop; reboot-survives + multi-hour watchdog
co-existence soak. *Deliverable:* power-on → Wi-Fi → IR API, no console, survives reboot. *Retire
first:* **does `hal` run standalone without luaworks/dbus?** (already probed in M1; here wire the
luaworks/dbus-minimal or Route B fallback if M1 showed it won't run bare) and **watchdog
co-existence** (no reboot loop over a soak).

**M6 — niceties (optional).** Decoded-protocol enrichment in learn (via `luaworks.so` codec),
DB import/export, bundled static web UI served from `/`, `RawSpiBackend` (Route B) behind the trait.

---

## 6. Top risks + retirement

| # | Risk | Why it bites | Retire |
|---|------|--------------|--------|
| R1 | **`hal` won't run standalone** (aborts without luaworks/dbus/`/etc/nonce`) | kills Route A; forces Route B (hard) | **M1:** start `hal -s` with dbus up under the bring-up script, watch it bind `:16716` and fire. Evidence says IR path is dbus/nonce-independent (those are BT/WLAN-only); dbus bus is started anyway. Fallback: bring up minimal dbus + only the luaworks bits hal needs, else Route B. |
| R2 | **`ports` bit→emitter map wrong** | "all emitters" default fires the wrong/no port | **M1:** fire 1/2/4/7, phone-camera each emitter; pin the map; set DB default accordingly. |
| R3 | **Multi-packet LTCP framing wrong** for >61 B blobs | real IR codes exceed one packet → corrupt sends | **M1/M4:** implement+verify single-packet first; for multi, Ghidra `formLtcpPacket`/`formParameter` or capture one real stock long send; checksum/seq-flag assertions on the wire. |
| R4 | **ir_cap result framing unknown** (streamed? end-marker?) | learn returns garbage/partial blobs | **M4:** live capture against `hal`, instrument the read loop until a known remote round-trips (learn→replay reproduces the press). |
| R5 | **Watchdog reboot loop** (hal heartbeat stops / OOM / `-w` + watchdog) | box reboots every 15 s | **M1/M5:** never pass `hal -w` while `watchdog` runs; seed `/var/watchdog/hal`; `vm.overcommit=1`; bound irapi memory (capped bodies, `max_db_bytes`, no unbounded learn buffering); multi-hour soak in M5. Kill switch: `touch /etc/nowatchdog`. |
| R6 | **No network path** (only `lo` from bare shell) | API unreachable; whole goal blocked | **M5 (net-up.sh):** umac stack → `ath0` sta → wpa_supplicant `-Dwext` → udhcpc; gate on `wpa_state=COMPLETED` + non-zero inet. Backend-agnostic (irapi runs regardless), so de-risk in parallel with M2–M4 on a wired/host path. Fallback transport: USB-ethernet gadget (`usb-gadget-console.md`). |
| R7 | **jffs2 write corruption** on power loss | DB loss / bricked config file | **M3:** temp+`sync_all`+atomic `rename`+dir fsync; startup recovers from `.tmp` or empty; power-yank test. |
| R8 | **Binary too big / serde bloat** under `opt-level=z` | overlay/RAM pressure | **M2:** zero runtime crates, hand-rolled HTTP/JSON; measure each addition; `nanoserde` only if hand-roll fails. Native ~0.8 MB vs ~4 MB overlay = ample. **Never UPX** (stub SIGTRAPs on this kernel). |
| R9 | **/dev nodes missing** (macOS extract dropped them) | hal/IR can't open devices | **M1:** `ls -l /dev/rfspi /dev/rffw /dev/watchdog`; if absent, `mknod /dev/rfspi c 10 59 ; mknod /dev/rffw c 10 60 ; mknod /dev/watchdog c 10 130`. Manifest says they're static in squashfs; devtmpfs likely auto-creates them. |

**Single most important next action:** execute **M1** — it simultaneously retires R1 (hal
standalone), R2 (`ports` map), R3-single-packet (LTCP framing), and R5 (watchdog), and is the
gate for everything else. Do it with a **blob captured via a one-time controlled boot of the stock
app** (or via M4's `/ir/ir_cap` once that lands), not by reimplementing the codec.

---

### Key files
- Build: `service/build-mips.sh`; clone `experiments/rustprobe/{Cargo.toml,.cargo/config.toml,src/main.rs}`.
- Bring-up: `service/standalone-ir-bringup.sh` (fold into `/mnt/data/irapi/boot/`).
- Upload: `tools/upload_file.py` + `tools/b64decode.lua` (zip→base64→UART→unzip→md5; **no UPX**).
- Stock RE sources (extract via `backups/sqextract-atheros-lzma.py backups/mtd3.bin <out>`):
  `usr/bin/hal`, `lib/firmware/cc2544.bin`, `lib/modules/2.6.31-g89d565c/cc2544.ko`,
  `opt/luaworks/tasks/hal/core/{ltcp,hbus}.lua`, `opt/luaworks/tasks/harmonyengine/core/{irsender,irmanager}.lua`.
- Context: `docs/ir-api-project.md`, `docs/hardware-specs.md`. Recovery: `backups/*.bin`
  (mtd1–mtd5 over U-Boot serial; **never mtd0/mtd6**).

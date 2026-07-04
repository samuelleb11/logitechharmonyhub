# Harmony remote — 2.4 GHz RF link reverse engineering

Goal: pair a **Logitech Harmony Smart Control remote** to our appliance over its 2.4 GHz RF link
and receive button presses — then map each button to an IR command — without Logitech's `hal`.

## The radio: cc2544

The hub's **cc2544** (TI 2.4 GHz RF SoC, firmware v39.00.0010) is the radio that talks to the
remote — *not* an IR part (an earlier note misfiled it). Its kernel driver `cc2544.ko` exposes two
misc devices and hides the raw SPI (which is bit-banged over the shared NOR-flash bus):

| Node | major,minor | Purpose |
|------|-------------|---------|
| `/dev/rffw`  | 10,60 | firmware load: `cat /lib/firmware/cc2544.bin > /dev/rffw` |
| `/dev/rfspi` | 10,59 | command/report channel |

There is also a separate **CSR8811 Bluetooth** radio (`hci_*` in hal) — not used for this remote.

## The `/dev/rfspi` protocol (reversed from `hal`)

`hal`'s `rf_msg_init` (`@0x42b86c`) does, in order:

1. `open("/dev/rfspi", O_RDWR)` — the fd is stored at `Gbase+0x3490`.
2. `pthread_mutex_init` ×2 (command + a seq counter, both at `Gbase+0x6f34`/`0x6f4c`).
3. **`rf_msg_write`** of one **7-byte init command** (`@0x42b908`, `a1=7`) — the buffer at `sp+0x1c`
   is `{u32, u16, u8}`, big-endian, so **byte 0 = the opcode**.
4. `pthread_create` — a reader thread that blocks on `read(/dev/rfspi)` and dispatches packets.

- **Send a command:** `write(/dev/rfspi, {opcode, args…})` — the driver clocks it out verbatim
  (`rf_msg_write @0x42add4`: mutex, bump 16-bit seq at `Gbase+0x6f6c`, `write()`).
- **Receive:** `read(/dev/rfspi)` **blocks** and returns exactly one packet. Unsolicited RF button
  presses arrive this way — the cc2544 raises GPIO14→IRQ46, `cc2544.ko` enqueues the packet, and
  `read()` dequeues it (`packet[0]` = type byte). *(driver fully reversed — see
  [ir-receive-learn-plan.raw.txt](ir-receive-learn-plan.raw.txt).)*

Our appliance is single-threaded (no futex), so instead of `hal`'s reader thread we use a dedicated
**process** (`irapi rf …`) that owns `/dev/rfspi` and `poll()`s it.

### cc2544 opcode table (`hal` `rf_msg_dump @0x461d84`)

`0x11` LEDS · `0x12` REPORTING · `0x13` IRCAMERA_CLOCK · `0x14` SPEAKER_ENABLE · `0x15` STATUS ·
`0x16` WRITE_MEM · `0x17` READ_MEM · `0x18` SPEAKER_DATA · `0x1a` IRCAMERA_ENABLE · `0x20` STATUS ·
`0x22` ACK · **`0x28+` = button REPORTs**. (SPEAKER_* are for remotes with a mic — not the Smart
Control.)

## Pairing

- **Remote side (confirmed):** hold **Menu + Mute** to put the remote into pairing/discovery mode.
- **Hub side:** the stock UI says "hold the hub reset button 30 s" — that makes the hub enter RF
  pairing mode. We replace that physical step with a software command to the cc2544.
- `hal` exposes it as the LTCP/HTTP method **`/rf/pairing`** (route table `@.data 0x462a70`,
  dispatch `@0x420f28`) — but the handler is a generic hbus-RPC thunk that calls a registered
  vtable method, so the exact cc2544 pairing-enable bytes are behind RPC/thread indirection.
  `libhal_dongle_device_add`/`_remove`, `libhal_get_paired_device_info`, `led_enable_pairing`,
  `PairingStart/Stop`, `/rf/unpair` are all present. **The exact pairing-enable command is easiest
  to confirm live** (see below).

## The air protocol is fully known (and UNENCRYPTED)

The remote's RF chip is a **Nordic nRF24LE1** (8051 + nRF24L01+ radio). The
[Harmoino project](https://github.com/joakimjalden/Harmoino) already reversed the whole link by
reading it with a bare nRF24L01+ — it is **not encrypted**:

- **2 Mbps**, **40-bit address**, **CRC-16**, dynamic payload length.
- Frequency-hops over **12 channels**: 5, 8, 14, 17, 32, 35, 41, 44, 62, 65, 71, 74.
- **Pairing**: the remote transmits a 22-byte packet to the shared address **`0xBB0ADCA575`**; the
  hub replies via ACK payload with the assigned 40-bit network address. (This matches the user
  procedure: hold **Menu+Mute** on the remote while the hub is in pairing mode.)
- **Button packet**: two ~10-byte packets per press; the button is a little-endian u32 at payload
  **offset 2** — and it is a **USB-HID usage** (low byte `0xC1` = keyboard page, `0xC3` = consumer
  page). e.g. `up`=0x00520 0C1 (HID Up), `ok`=0x005800C1 (Enter), `vol_up`=0x0000E9C3, `mute`=0x0000E2C3,
  `play`=0x0000B0C3. The full table for this remote is baked into `service/irapi/src/rf.rs`
  (`BUTTONS`), so once we see a packet we name the button.
- Checksum = last byte (packet sums to 0 mod 256); 5-byte keepalives while held/idle.

**So the only thing left to confirm live is the cc2544↔`/dev/rfspi` framing** — i.e. how the hub's
radio wraps that nRF24 payload before `read()` hands it to us (it may prepend a type/opcode byte).
`rf.rs::decode_button` scans every 4-byte window for a known command_id, so it decodes regardless of
a leading byte. We also confirm whether pairing is autonomous once reporting is enabled, or needs an
explicit enable command.

### The tools we built for this
- `tools/mipsdis.py` — capstone big-endian-MIPS RE helper (string/xref/pointer/symbol/disasm with
  MIPS GOT→dynsym resolution), used against the extracted `hal` + `cc2544.ko`.
- `service/irapi/src/rf.rs` + the **`irapi rf`** CLI: `fw`, `sniff`, `pair`, `send`.

### Live experiment (run on the hub over ssh/devshell)
```sh
# 1. firmware is loaded at boot by rcS.local; sniff raw RF packets while pressing buttons:
irapi rf sniff --secs 60          # press remote buttons — do reports arrive as-is?

# 2. if nothing arrives, enable reporting first (candidate opcode 0x12) and retry:
irapi rf sniff --secs 60 --cmd 12ff

# 3. pairing: trigger + hold Menu+Mute on the remote:
irapi rf pair --secs 30           # (add --cmd <pairing-bytes> once identified)

# 4. send a probe command and see the reply (e.g. STATUS 0x15):
irapi rf send --cmd 15
```
Each captured packet prints as `[len] KIND hex…`; `0x28+` = a button report. From a handful of
presses we get the button→byte map, then wire it to IR.

## Next: button → IR mapping (the goal)

Target button set (Harmony Smart Control): Off; 3 Activity; Prev/Rew/Play/FF/Next/Rec/Pause/Stop;
Red/Green/Yellow/Blue; DVR/Guide/Info; Exit/Menu; D-pad Up/Down/Left/Right/OK; Vol±; Ch/Pg±;
Mute/Back; 0-9, "‑‑", E. Plan: `irapi rf` process reads reports → looks up `/cache/remotemap.json`
(button code → `{device,function}` / raw / AC) → fires via `i2s::blast`; web + HA UI to bind each
button ("press a button to assign it").

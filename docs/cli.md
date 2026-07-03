# `irapi` CLI reference

`irapi` is the standalone IR appliance binary for the re-flashed Harmony Hub — a
single-threaded, static big-endian mips-musl Rust binary that drives the AR9331
I2S peripheral directly (Logitech's `hal` is not used). Source:
[`service/irapi/src/main.rs`](../service/irapi/src/main.rs).

Every subcommand is dispatched from the `match` in `fn run()`. There is no `clap`:
flags are parsed by hand and accept **both** `--flag value` and `--flag=value`
forms. Unknown flags are ignored; missing required flags exit `2`.

```
irapi <command> [--flags…]
```

Run `irapi` with no command (or `-h`/`--help`) for the built-in usage summary.

## How it runs on the appliance

On the device, `irapi` is **not** invoked by hand — it is launched at boot by the
overlay init script [`service/rcS.local`](../service/rcS.local) from `/root/irapi`:

| Service | Launched as |
| --- | --- |
| Web API + UI | `irapi serve --port 80` |
| Untethered dev shell | `irapi devshell --port 2222 --token harmonydev` |
| DHCP (AP-fallback only) | `irapi dhcpd --iface ath1 --server 192.168.4.1 --start 192.168.4.10 --count 20` |

`serve` and `devshell` start on every boot; `dhcpd` starts only when Wi-Fi
station mode fails and the hub falls back to its own `Harmony-Setup` AP. All other
verbs below are run manually over the UART/dev shell for testing and bring-up.

> ⚠️ **HARDWARE HAZARD — `peek` / `poke` / `regs`.** These access physical MMIO
> through `/dev/mem`. **Reading a clock-gated register HANGS the SoC bus** and the
> hub needs a **power cycle** to recover — there is no soft recovery. `poke` can
> also brick a running session by writing a wrong register. Only touch documented
> IR/clock registers, and never `peek` a block whose clock you have not confirmed
> is enabled.

---

# Appliance & everyday

## `serve` — the appliance (HTTP API + web UI)

Serve the JSON API and the self-contained web UI (the whole product). Runs forever.

```
irapi serve [--port 80]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--port` | `80` | TCP port to listen on |

```sh
irapi serve --port 80
```

## `fire` — transmit a code from the offline library

Look up a device/function in the bundled offline DB and blast it via the direct
I2S driver.

```
irapi fire --device ID --function NAME [--select 7]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--device` | *(required)* | Library device id (see `irapi codes`) |
| `--function` | *(required)* | Function/button name on that device |
| `--select` | `7` | Emitter-select bitmask (bits 0/1/2 → the three IR outputs; `7` = all) |

Missing `--device` or `--function` exits `2`; an unknown code exits `2`.

```sh
irapi fire --device samsung_tv_BN59 --function KEY_POWER --select 7
```

## `ac` — Midea/Danby air-conditioner encoder

Encode a climate state on the fly (no DB entry needed) and transmit it. Carrier is
fixed at 38 kHz; `--temp` is clamped to 17–31 °C by the encoder.

```
irapi ac [--power on|off] [--mode cool|heat|dry|fan|auto] [--fan auto|low|medium|high] [--temp 22] [--select 7] [--dry-run]
```

| Flag | Default | Accepted values |
| --- | --- | --- |
| `--power` | `on` | `on` (anything except…) / `off`, `0`, `false` |
| `--mode` | `cool` | `cool`, `heat`, `dry`, `fan`/`fan_only`, `auto` (fallback) |
| `--fan` | `auto` | `low`, `medium`/`med`, `high`, `auto` (fallback) |
| `--temp` | `22` | °C (clamped 17–31) |
| `--select` | `7` | Emitter-select bitmask |
| `--dry-run` | off | Print the 6 encoded frame bytes and exit **without** transmitting |

```sh
irapi ac --power on --mode cool --fan high --temp 22
# preview the frame bytes only:
irapi ac --mode heat --temp 25 --dry-run
```

## `learn` — capture a physical remote button

Listen on the IR receiver, decode the button to a mark/space timing sequence, and
print it. With **both** `--device` and `--function`, also save it to the overlay DB
so it is browseable and fireable like any bundled code.

```
irapi learn [--secs 15] [--ups 0.094] [--device ID --function NAME] [--type Custom] [--brand Learned] [--model ID]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--secs` | `15` | Seconds to wait for a button press |
| `--ups` | `0.094` | µs-per-sample calibration (`learn::US_PER_SAMPLE`) |
| `--device` | *(unset)* | If given with `--function`, save the capture under this id |
| `--function` | *(unset)* | Function/button name to save it as |
| `--type` | `Custom` | Device type for the saved entry |
| `--brand` | `Learned` | Brand for the saved entry |
| `--model` | *(= `--device`)* | Model for the saved entry |

Prints the decoded carrier and µs interval list. Saving only happens when both
`--device` and `--function` are supplied.

```sh
# just print the timing:
irapi learn --secs 20
# capture and save as a fireable code:
irapi learn --device my_soundbar --function KEY_VOLUP --brand Sony
```

## `codes` — list the library

With no flag, list every bundled device (`id`, type, brand, model). With
`--device`, list that device's function names.

```
irapi codes [--device ID]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--device` | *(unset)* | List functions of this device instead of all devices |

```sh
irapi codes                       # all devices
irapi codes --device samsung_tv_BN59   # its functions
```

---

# Developer / reverse-engineering

These verbs poke the hardware directly. `hal` must be stopped first for any TX
path (it holds `/dev/i2s` exclusively) — on this appliance `hal` is already never
started (see [`service/rcS.local`](../service/rcS.local)).

## `i2sraw` — blast an arbitrary mark/space sequence

One-shot transmit of any host-computed µs timing list, so it can drive **any** IR
protocol. `--us` alternates mark,space,mark,space,… in microseconds.

```
irapi i2sraw --us m,s,m,s,… [--carrier 38000] [--select 7] [--bitmul 1] [--hold]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--us` | *(required)* | Comma-separated µs list, starting with a mark |
| `--carrier` | `38000` | Carrier frequency (Hz; accepts `0x…`) |
| `--select` | `7` | Emitter-select bitmask |
| `--bitmul` | `1` | Bitstream upsample factor |
| `--hold` | off | Use non-blocking Hold finish instead of the default blocking Drain (returns after one pass) |

Missing/empty/malformed `--us` exits `2`.

```sh
# a NEC-ish frame:
irapi i2sraw --us 9000,4500,560,560,560,1690,560,560 --carrier 38000
```

## `i2stest` — continuous carrier (camera check)

Emit one continuous carrier MARK so the emitter is visible on a phone camera. Keep
`--ms` small (<~400 ms at `--bitmul 16`) — the buffer is 16× larger and can
overflow the ~393 KB DMA ring.

```
irapi i2stest [--carrier 38000] [--ms 1000] [--select 7] [--bitmul 16] [--drain]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--carrier` | `38000` | Carrier frequency (Hz; accepts `0x…`) |
| `--ms` | `1000` | Duration of the MARK, milliseconds |
| `--select` (alias `--ports`) | `7` | Emitter-select bitmask |
| `--bitmul` (alias `--clkmul`) | `16` | Bitstream upsample (compensates the ~16× fast BCLK on minimal boot) |
| `--drain` | off | Opt into the **blocking** Drain finish (only once the divider is known-good); default is the safe non-blocking Hold |

```sh
irapi i2stest --carrier 38000 --ms 300
```

## `i2scap` — dump a raw IR capture (RX calibration)

Capture from `/dev/i2s` `O_RDONLY`, run-length decode it, and print the runs plus
µs-per-sample calibration hints against known leader lengths. Used to confirm RX
works and calibrate `learn --ups`.

```
irapi i2scap [--secs 8] [--start]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--secs` | `8` | Seconds to listen |
| `--start` | off | Send the RX start-kick before capturing |

Exits `1` if the line stayed idle (nothing captured).

```sh
irapi i2scap --secs 8      # then press a known remote at the hub
```

## `i2sgpiosweep` — identify the emitter-select GPIO

Blast the same carrier burst through each candidate emitter-select value
(`0`=DMA-only, `1`→GPIO16, `2`→GPIO28, `4`→GPIO13, `7`=all three) with a pause
between, to find which bit drives the physical blaster. Takes no `--select` (it
sweeps them all).

```
irapi i2sgpiosweep [--carrier 38000] [--ms 300] [--bitmul 16]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--carrier` | `38000` | Carrier frequency (Hz) |
| `--ms` | `300` | Burst length per candidate, milliseconds |
| `--bitmul` | `16` | Bitstream upsample factor |

```sh
irapi i2sgpiosweep --ms 300
```

## `peek` — read MMIO registers ⚠️

Read one or more 32-bit registers via `/dev/mem`. **See the hardware-hazard warning
above — reading a clock-gated register hangs the SoC.**

```
irapi peek --addr 0x… [--count 1]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--addr` | *(required)* | Base physical address (hex or decimal) |
| `--count` | `1` | Number of consecutive 32-bit words (address += 4 each) |

Missing `--addr` exits `2`.

```sh
irapi peek --addr 0xb8040000 --count 4
```

## `poke` — write an MMIO register ⚠️

Write a 32-bit register via `/dev/mem`. **Same hazard — a wrong write can wedge the
SoC and require a power cycle.**

```
irapi poke --addr 0x… --val 0x…
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--addr` | *(required)* | Physical address (hex or decimal) |
| `--val` | *(required)* | 32-bit value to write (hex or decimal) |

Missing `--addr` or `--val` exits `2`.

```sh
irapi poke --addr 0xb8040000 --val 0x1
```

## `regs` — dump the IR-relevant registers ⚠️

Peek the fixed table of AR9331 registers relevant to IR TX
(`mem::IR_REGS`) and print each with its name and expected-value note. Takes no
flags. **Reads `/dev/mem` — the same clock-gating hazard applies.**

```
irapi regs
```

```sh
irapi regs
```

## `devshell` — untethered TCP shell (trusted LAN only)

Token-authed, single-connection-at-a-time command server (the device has no
dropbear). The client sends the token as the first line, then one shell command per
line; `exit` closes the connection.

```
irapi devshell [--port 2222] [--token harmony]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--port` | `2222` | TCP port to listen on |
| `--token` | `harmony` | Auth token (first line must match) |

> ⚠️ **Security.** This is a **dev tool for a trusted LAN only** — plaintext, no
> encryption, minimal auth. The in-code default token is `harmony`, but
> [`service/rcS.local`](../service/rcS.local) launches it with `--token harmonydev`;
> **change that token** before deploying anywhere untrusted.

```sh
irapi devshell --port 2222 --token my-secret
# from a host:
printf 'my-secret\nuname -a\nexit\n' | nc <hub-ip> 2222
```

## `dhcpd` — tiny DHCP server for AP-fallback mode

Minimal single-threaded UDP DHCP server (the device has no `udhcpd`). Serves a
small sequential pool on **one** interface (`SO_BINDTODEVICE`), so it cannot answer
DHCP on the real LAN. Used only when the hub falls back to its own AP.

```
irapi dhcpd [--iface ath1] [--server 192.168.4.1] [--start 192.168.4.10]
            [--count 20] [--mask 255.255.255.0] [--lease 3600]
            [--port 67] [--reply-port 68]
```

| Flag | Default | Meaning |
| --- | --- | --- |
| `--iface` | `ath1` | Interface to bind to (SO_BINDTODEVICE) |
| `--server` | `192.168.4.1` | Server / router / DNS IP handed out |
| `--start` | `192.168.4.10` | First IP in the pool |
| `--count` | `20` | Pool size |
| `--mask` | `255.255.255.0` | Subnet mask offered |
| `--lease` | `3600` | Lease time, seconds |
| `--port` | `67` | UDP listen port |
| `--reply-port` | `68` | UDP reply port (client) |

```sh
irapi dhcpd --iface ath1 --server 192.168.4.1 --start 192.168.4.10 --count 20
```

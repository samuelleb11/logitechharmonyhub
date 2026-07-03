# Harmony Hub IR Appliance — HTTP API Reference

The `irapi` firmware serves a self-contained web UI plus a small JSON API on **port 80**
(`irapi serve [--port 80]`). Every route below is registered in the `fn route(...)` match in
[`service/irapi/src/web.rs`](../service/irapi/src/web.rs).

Conventions used throughout:

- Base URL is `http://<hub-ip>` (port 80). Replace `<hub-ip>` with the hub's LAN address
  (see [`GET /api/status`](#get-apistatus--get-apihealth)).
- All JSON responses set `Content-Type: application/json` and `Cache-Control: no-store`;
  connections are `Connection: close` (one request per TCP connection).
- **Error envelope:** failing JSON endpoints return `{"ok": false, "error": "<message>"}` with a
  non-2xx HTTP status. Success envelopes vary per endpoint (documented below).
- **`select` is an emitter bitmask, `0..7`.** Bit 0 (`1`) = emitter port0/GPIO16, bit 1 (`2`) =
  port1/GPIO28, bit 2 (`4`) = port2/GPIO13. `7` = all three emitters (the default), `0` = none
  (DMA only, no physical emit). Applies to every IR/AC send.
- Unknown method/path → `404 Not Found`, `text/plain` body `not found`.

## Endpoint index

| Method | Path | Purpose |
|---|---|---|
| GET | `/` , `/index.html` | Self-contained web UI (HTML) |
| GET | `/api/status` , `/api/health` | Device status |
| GET | `/api/ir/types` | Library: device categories |
| GET | `/api/ir/brands?type=` | Library: brands in a type |
| GET | `/api/ir/devices[?type=&brand=]` | Library: devices |
| GET | `/api/ir/functions?device=` | Library: a device's buttons |
| POST | `/api/ir/send` | Send a code (DB lookup **or** raw µs) |
| POST | `/api/ac/send` | Send a Midea/Danby AC climate state |
| POST | `/api/ir/learn` | Capture a physical remote button |
| POST | `/api/ir/learn/save` | Persist a learned code |
| POST | `/api/ir/forget` | Delete a learned code/device |
| GET | `/api/ha/rest_command.yaml` | Home Assistant helper config (YAML) |
| GET | `/api/wifi/scan` | Scan for Wi-Fi networks |
| POST | `/api/wifi/connect` | Join a Wi-Fi network (reboots) |
| POST | `/api/ota?token=` | Replace firmware (reboots) |
| POST | `/api/ota/db?token=` | Replace the code DB (hot reload) |
| POST | `/api/reboot` | Reboot |
| POST | `/api/factory-reset` | Wipe Wi-Fi + codes, reboot to setup AP |

---

## Status

### `GET /api/status` · `GET /api/health`

Current network/IR state. Both paths are identical.

**Response `200 OK`:**

```json
{
  "mode": "station",
  "ssid": "MyWiFi",
  "ip": "192.168.1.42",
  "ir": "ready",
  "uptime": "3812",
  "version": "0.1.0"
}
```

| Field | Type | Notes |
|---|---|---|
| `mode` | string | `"AP"` (setup hotspot), `"station"` (joined Wi-Fi), or `"down"` |
| `ssid` | string | Active SSID (AP or station), empty when down |
| `ip` | string | IP of the active interface (`ath1` in AP mode, `ath0` in station) |
| `ir` | string | `"ready"` if `/dev/i2s` exists, else `"no /dev/i2s"` |
| `uptime` | string | Seconds of uptime (integer as string) |
| `version` | string | `irapi` build version (`CARGO_PKG_VERSION`) |

```bash
curl http://<hub-ip>/api/status
```

---

## Library browse

The offline IR code library (a curated Flipper-IRDB subset at `/cache/irdb.txt` plus any learned
codes) is browsable as **type → brand → device → function**.

### `GET /api/ir/types`

All device categories.

**Response `200 OK`:** `{"types": ["TVs", "ACs", ...]}`

```bash
curl http://<hub-ip>/api/ir/types
```

### `GET /api/ir/brands?type=<type>`

Brands within a type.

| Query param | Type | Notes |
|---|---|---|
| `type` | string | Category from `/api/ir/types`. Missing/empty → empty list |

**Response `200 OK`:** `{"brands": ["Samsung", "LG", ...]}`

```bash
curl 'http://<hub-ip>/api/ir/brands?type=TVs'
```

### `GET /api/ir/devices[?type=<type>&brand=<brand>]`

Devices, optionally filtered by `type` and/or `brand`. Both params are optional; omit both to list
everything.

**Response `200 OK`:**

```json
{
  "devices": [
    { "id": "tv_samsung_aa59", "type": "TVs", "brand": "Samsung", "model": "AA59-00666A" }
  ]
}
```

```bash
curl 'http://<hub-ip>/api/ir/devices?type=TVs&brand=Samsung'
```

### `GET /api/ir/functions?device=<id>`

Function (button) names for a device.

| Query param | Type | Notes |
|---|---|---|
| `device` | string | Device `id` from `/api/ir/devices`. Unknown/empty → empty list |

**Response `200 OK`:** `{"functions": ["Power", "Vol_up", "Vol_dn", ...]}`

```bash
curl 'http://<hub-ip>/api/ir/functions?device=tv_samsung_aa59'
```

---

## Send

### `POST /api/ir/send`

Fire an IR code through the direct-I2S transmit path. The JSON body is **one of two shapes**.

**Shape A — DB lookup (device + function):**

| Field | Type | Default | Notes |
|---|---|---|---|
| `device` | string | — | Device `id` (required) |
| `function` | string | — | Button name (required). `command` is accepted as an alias (for Home Assistant) |
| `select` | int | `7` | Emitter bitmask `0..7` |

**Shape B — raw waveform (`raw_us`):**

| Field | Type | Default | Notes |
|---|---|---|---|
| `raw_us` | int[] | — | Alternating mark, space durations in **µs**, starting with a **mark** (index 0 = mark). Must be non-empty |
| `carrier` | int | `38000` | Carrier frequency in Hz |
| `duty` | int | `33` | Duty cycle percent (forced to ≥ 1) |
| `select` | int | `7` | Emitter bitmask `0..7` |

If `raw_us` is present it takes precedence; otherwise `device` + `function` are used.

**Response `200 OK`:** `{"ok": true, "emitted": <int>}` (`emitted` = number of mark/space intervals sent).

**Errors:**

| Status | Body `error` | Cause |
|---|---|---|
| `400 Bad Request` | `empty raw_us` | `raw_us` given but decoded to nothing |
| `400 Bad Request` | `need raw_us, or device+function` | Neither shape satisfied |
| `404 Not Found` | `no such device/function` | DB lookup failed |
| `502 Bad Gateway` | driver message | I2S emit failed |

```bash
# DB lookup
curl -X POST http://<hub-ip>/api/ir/send \
  -H 'Content-Type: application/json' \
  -d '{"device":"tv_samsung_aa59","function":"Power","select":7}'

# raw waveform (NEC-ish leader + a bit, emitter port0 only)
curl -X POST http://<hub-ip>/api/ir/send \
  -H 'Content-Type: application/json' \
  -d '{"raw_us":[9000,4500,560,560],"carrier":38000,"duty":33,"select":1}'
```

---

## AC (climate)

### `POST /api/ac/send`

Drive a Midea/Danby air conditioner from a climate state. The full 2-frame IR waveform is encoded
on the fly (38 kHz) — no DB entry required.

| Field | Type | Default | Notes |
|---|---|---|---|
| `power` | string | on | Off when `"off"`, `"0"`, or `"false"`; otherwise on |
| `mode` | string | `"cool"` | `cool`, `heat`, `dry`, `fan` (aka `fan_only`), or `auto`; any other value → `auto` |
| `fan` | string | `"auto"` | `auto`, `low`, `medium` (aka `med`), or `high` |
| `temp` | int | `22` | Target °C, clamped to `17..31` |
| `select` | int | `7` | Emitter bitmask `0..7` |

**Response `200 OK`:**

```json
{ "ok": true, "power": true, "temp": 22, "emitted": 200 }
```

**Error `502 Bad Gateway`:** `{"ok": false, "error": "<driver message>"}` (I2S emit failed).

```bash
curl -X POST http://<hub-ip>/api/ac/send \
  -H 'Content-Type: application/json' \
  -d '{"power":"on","mode":"cool","fan":"high","temp":21,"select":7}'
```

---

## Learn

### `POST /api/ir/learn`

Capture a physical remote button and return the decoded waveform. **Blocks the (single-threaded)
server** until a button is pressed or the timeout elapses — aim the remote at the hub's front.

| Field | Type | Default | Notes |
|---|---|---|---|
| `timeout_ms` | int | `15000` | Capture window in ms. Effective wait = `max(2, timeout_ms/1000)` seconds |

**Response `200 OK`:**

```json
{ "ok": true, "carrier": 38000, "count": 67, "us": [9000, 4500, 560, 560, 560, 1690] }
```

| Field | Type | Notes |
|---|---|---|
| `carrier` | int | Assumed carrier in Hz (38000) |
| `count` | int | Number of entries in `us` |
| `us` | int[] | Alternating mark, space µs, starting with a mark (feed straight into `learn/save`) |

**Error `504 Gateway Timeout`:** `{"ok": false, "error": "..."}` (nothing received, or no clean frame decoded).

```bash
curl -X POST http://<hub-ip>/api/ir/learn \
  -H 'Content-Type: application/json' \
  -d '{"timeout_ms":15000}'
```

### `POST /api/ir/learn/save`

Persist a learned code as a custom device (then browsable/fireable like any bundled code).
Stored on the `/cache` overlay (`custom.json`) and hot-reloaded.

| Field | Type | Default | Notes |
|---|---|---|---|
| `function` | string | — | Button name (**required**) |
| `us` | int[] | — | Mark/space µs, mark-first (**required**, non-empty) — e.g. straight from `/api/ir/learn` |
| `carrier` | int | `38000` | Carrier in Hz |
| `type` | string | `"Custom"` | Library category |
| `brand` | string | `"Learned"` | Library brand |
| `device` | string | auto `custom-NNNN` | Device id; reused across saves to add buttons to one device |
| `model` | string | = `device` | Display model name |

**Response `200 OK`:** `{"ok": true, "device": "custom-0001", "function": "Power"}`

**Errors:**

| Status | Body `error` |
|---|---|
| `400 Bad Request` | `need function + us` |
| `500 Internal Server Error` | write/persist message |

```bash
curl -X POST http://<hub-ip>/api/ir/learn/save \
  -H 'Content-Type: application/json' \
  -d '{"function":"Power","carrier":38000,"us":[9000,4500,560,560],"type":"TVs","brand":"MyTV","device":"custom-0001","model":"Living Room TV"}'
```

### `POST /api/ir/forget`

Delete a learned code. Omit `function` to remove the **entire** learned device; supply it to remove
just that one button (the device is dropped if it becomes empty). Only affects learned devices —
**the bundled DB is read-only.** Backs Home Assistant's `remote.delete_command`.

| Field | Type | Notes |
|---|---|---|
| `device` | string | Learned device id (**required**) |
| `function` | string | Optional; omit to delete the whole device |

**Response `200 OK`:** `{"ok": true, "removed": true}`

**Errors:**

| Status | Body `error` |
|---|---|
| `400 Bad Request` | `need device` |
| `404 Not Found` | `no matching learned device/function` |
| `500 Internal Server Error` | persist message |

```bash
# forget one button
curl -X POST http://<hub-ip>/api/ir/forget \
  -H 'Content-Type: application/json' \
  -d '{"device":"custom-0001","function":"Power"}'

# forget the whole learned device
curl -X POST http://<hub-ip>/api/ir/forget \
  -H 'Content-Type: application/json' \
  -d '{"device":"custom-0001"}'
```

---

## Home Assistant helper

### `GET /api/ha/rest_command.yaml`

Returns a ready-to-paste Home Assistant `rest_command:` block (`Content-Type: text/yaml`) that fires
any code by `device` + `function` through `POST /api/ir/send`. The hub substitutes its own `ath0` IP
into the URL (falls back to `<hub-ip>` if unknown). The generated command takes an optional `select`
(defaults to `7`).

```bash
curl http://<hub-ip>/api/ha/rest_command.yaml
```

Emitted YAML (IP filled in):

```yaml
rest_command:
  ir_send:
    url: "http://192.168.1.42/api/ir/send"
    method: POST
    content_type: "application/json"
    payload: '{"device":"{{ device }}","function":"{{ function }}","select":{{ select|default(7) }}}'
```

---

## OTA (over-the-air update)

> **Both OTA endpoints require `?token=`** matching the shared secret (default `harmonydev`,
> defined as `OTA_TOKEN` in [`web.rs`](../service/irapi/src/web.rs) — **change it before deploying**).
> A wrong/missing token returns `403 Forbidden` `{"ok": false, "error": "bad or missing ?token="}`.
> See also [`tools/ota.py`](../tools/ota.py).

### `POST /api/ota?token=<token>`

Replace the running firmware. The request body is the **raw new MIPS ELF binary**. The previous
binary is kept at `/root/irapi.prev`; the swap is done via `rename` (safe while running); then the
hub reboots ~1 s later.

**Validation:** body must be ≥ 100 000 bytes and start with the `\x7fELF` magic.

**Response `200 OK`:**

```json
{ "ok": true, "bytes": 812345, "detail": "firmware flashed; rebooting (prev kept at /root/irapi.prev)" }
```

**Errors:**

| Status | Body `error` |
|---|---|
| `403 Forbidden` | `bad or missing ?token=` |
| `400 Bad Request` | `body is not a firmware ELF (>=100KB, 0x7fELF magic)` |
| `500 Internal Server Error` | write/swap message |

```bash
curl -X POST 'http://<hub-ip>/api/ota?token=harmonydev' \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @service/irapi/target/mips-unknown-linux-musl/release/irapi
```

### `POST /api/ota/db?token=<token>`

Replace the code database at `/cache/irdb.txt`. The body is a full `irdb.txt` file. **Hot-reloaded,
no reboot.**

**Validation:** body must be ≥ 20 bytes and start with `D\t` (a tab-separated device record).

**Response `200 OK`:** `{"ok": true, "bytes": <int>, "devices": <int>}` (`devices` = count after reload).

**Errors:**

| Status | Body `error` |
|---|---|
| `403 Forbidden` | `bad or missing ?token=` |
| `400 Bad Request` | `body doesn't look like an irdb.txt (starts with 'D\t')` |
| `500 Internal Server Error` | write/swap message |

```bash
curl -X POST 'http://<hub-ip>/api/ota/db?token=harmonydev' \
  -H 'Content-Type: text/plain' \
  --data-binary @service/irapi/codes/irdb.txt
```

---

## Wi-Fi

### `GET /api/wifi/scan`

Scan for nearby networks (best effort; needs a station VAP on `ath0` — in AP-only setup mode this may
return an empty list and the user types the SSID manually).

**Response `200 OK`:**

```json
{
  "networks": [
    { "ssid": "MyWiFi", "signal": "-52", "enc": true }
  ]
}
```

| Field | Type | Notes |
|---|---|---|
| `ssid` | string | Network name (duplicates de-duplicated) |
| `signal` | string | Signal level / quality as reported by `iwlist` |
| `enc` | bool | `true` if the network is encrypted |

```bash
curl http://<hub-ip>/api/wifi/scan
```

### `POST /api/wifi/connect`

Write `wpa_supplicant.conf` for the given network and **reboot ~2 s later** to apply.

| Field | Type | Notes |
|---|---|---|
| `ssid` | string | Network name (**required**, trimmed). SSID/PSK control chars are stripped before writing config |
| `psk` | string | Passphrase; **empty/omitted → open network** (`key_mgmt=NONE`) |

**Response `200 OK`:** `{"ok": true, "detail": "saved; rebooting to apply"}`

**Errors:**

| Status | Body `error` |
|---|---|
| `400 Bad Request` | `ssid required` |
| `500 Internal Server Error` | `write failed` |

```bash
curl -X POST http://<hub-ip>/api/wifi/connect \
  -H 'Content-Type: application/json' \
  -d '{"ssid":"MyWiFi","psk":"s3cret"}'
```

---

## Maintenance

### `POST /api/reboot`

Reboot the hub (responds first, then reboots ~1 s later).

**Response `200 OK`:** `{"ok": true}`

```bash
curl -X POST http://<hub-ip>/api/reboot
```

### `POST /api/factory-reset`

Drop user data — deletes `/etc/wifi/wpa_supplicant.conf` and `/root/irdb.json` — then reboot ~2 s
later into the setup access-point. The appliance itself (`rcS.local`, `irapi`) is kept.

**Response `200 OK`:**

```json
{ "ok": true, "detail": "erased Wi-Fi + codes; rebooting into setup AP" }
```

```bash
curl -X POST http://<hub-ip>/api/factory-reset
```

---

## Web UI

### `GET /` · `GET /index.html`

Serves the self-contained management web UI (`Content-Type: text/html`) — the same page bundled into
the firmware. Not a JSON endpoint.

---

> **Security:** the entire API is **unauthenticated** except the two OTA endpoints (shared
> `?token=`), and is intended only for a trusted LAN.

# Harmony IR Blaster — Home Assistant integration

Control the standalone **Harmony IR appliance** (a reflashed Logitech Harmony Hub that blasts and
learns IR straight from `/dev/i2s`, no Logitech cloud) from Home Assistant — fully UI-configured.

> This is a Home Assistant **integration** (custom component), *not* an add-on. Add-ons run
> containers on the HA host; this talks to your appliance over the local network.

## What you get

- **Remote** entity — fire any code from the appliance's on-device library, and **learn** /
  **delete** codes natively through HA (`remote.learn_command` / `remote.delete_command`).
- **Buttons** — add any library device in the options UI and get one button per function.
- **Climate** — optional Midea/Danby **AC thermostat** (temperature, mode, fan), encoded on the
  appliance. Enable it under the integration's options.
- **Diagnostic sensors** — hub status, Wi-Fi SSID, IP, firmware version, IR subsystem, and last
  boot, grouped under the hub device.
- **Availability** — every entity greys out when the appliance is unreachable (30 s status poll).
- **Reconfigure** — change the hub's IP from the entry menu without re-adding.
- **Diagnostics** — one-click support bundle (host/IP redacted).
- **`harmony_ir.send_raw`** service — blast an arbitrary mark/space µs sequence.

## Install

### Manual (recommended)
Copy [`custom_components/harmony_ir/`](custom_components/harmony_ir/) into your Home Assistant
`config/custom_components/`, restart HA, then **Settings → Devices & Services → Add Integration →
Harmony IR Blaster** and enter the hub's IP.

### HACS
HACS expects an integration at `custom_components/<domain>/` in the **repo root**, but here it lives
under `homeassistant/`. To install via HACS, add a fork/repo that has `custom_components/harmony_ir/`
at its root as a custom **Integration** repository. (The manual method above always works.)

### Zero-code alternative (no HACS)
The appliance also serves ready-to-paste `rest_command` YAML at `http://<hub-ip>/api/ha/rest_command.yaml`.

## Configure (all in the UI)

**Settings → Devices & Services → Harmony IR Blaster → Configure:**
- **Add a device from the library** — browse type → brand → model; each function becomes a button.
- **Remove a device** — drop devices you added.
- **Air conditioner (Midea/Danby)** — toggle the AC climate entity on/off.

## Usage

Fire a library code (browse device ids / function names in the hub web UI at `http://<hub-ip>/`):
```yaml
service: remote.send_command
target: { entity_id: remote.harmony_ir_blaster }
data:
  device: "tv_samsung_samsung"   # a DB device id
  command: ["Power"]             # one or more function names
  num_repeats: 1
  delay_secs: 0.4
```

Learn a code from a physical remote (a notification prompts you to press the button):
```yaml
service: remote.learn_command
data:
  entity_id: remote.harmony_ir_blaster
  device: "bedroom_tv"           # any name; created on first learn
  command: ["Power", "VolumeUp"] # learned one at a time
```
Delete a learned code:
```yaml
service: remote.delete_command
data: { entity_id: remote.harmony_ir_blaster, device: "bedroom_tv", command: ["Power"] }
```

Air conditioner — a normal thermostat card, or:
```yaml
service: climate.set_temperature
target: { entity_id: climate.harmony_ir_blaster_air_conditioner }
data: { temperature: 22, hvac_mode: "cool" }
```

Raw timing (manual/advanced):
```yaml
service: harmony_ir.send_raw
data:
  entity_id: remote.harmony_ir_blaster
  raw_us: [9000, 4500, 560, 560, 560, 1690, 560, 40000]
  carrier: 38000
```

## Appliance API used
`GET /api/status`; `GET /api/ir/{types,brands,devices,functions}`; `POST /api/ir/send`
(`{device,function[,select]}` or `{raw_us,carrier[,select]}`); `POST /api/ac/send`;
`POST /api/ir/{learn,learn/save,forget}`. See the main project docs.

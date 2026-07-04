//! rf.rs — Harmony remote 2.4 GHz RF link via the cc2544 radio, straight from Rust.
//!
//! The cc2544 (TI 2.4 GHz SoC) is the radio that talks to the Harmony remote. Its kernel driver
//! `cc2544.ko` exposes `/dev/rfspi` (misc 10,59) and `/dev/rffw` (misc 10,60) and hides the raw
//! SPI. Reversed from the stock `hal` (rf_msg_init/rf_msg_write) + the driver:
//!   - firmware must be loaded first:  `cat /lib/firmware/cc2544.bin > /dev/rffw`
//!   - `open("/dev/rfspi", O_RDWR)` then `write({opcode, args…})` sends a command to the radio.
//!   - `read(/dev/rfspi)` BLOCKS and returns exactly one packet the radio delivered — unsolicited
//!     RF button presses arrive this way (driver enqueues on GPIO14→IRQ46). packet[0] = type byte.
//!
//! cc2544 opcode table (hal rf_msg_dump @0x461d84): 0x11 LEDS, 0x12 REPORTING, 0x15 STATUS,
//! 0x16 WRITE_MEM, 0x17 READ_MEM, 0x1a IRCAMERA_ENABLE, 0x20 STATUS, 0x22 ACK, 0x28+ = button REPORTs.
//!
//! Pairing (user side): hold **Menu+Mute** on the remote; the hub must be in pairing mode. We drive
//! the hub side from software (this module) instead of the stock "hold reset 30 s".

use crate::json::{self, Value};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;

// alarm(2) for a hard capture timeout: SIGALRM's default action terminates the process, ending a
// blocked read() (the cc2544 driver has no non-blocking read / poll). Avoids a libc crate dep.
extern "C" {
    fn alarm(seconds: u32) -> u32;
    fn getpid() -> i32;
}

/// Files the persistent `rf listen` daemon and the web UI use to coordinate (single-threaded, so we
/// talk over the /cache overlay instead of shared memory).
pub const LAST_PATH: &str = "/cache/rf_last.json"; // last decoded press (+ monotonic seq) for the UI/HA
pub const PID_PATH: &str = "/cache/rf_listen.pid"; // the listener's pid (so /api/rf/pair can signal it)
pub const PAIR_REQ_PATH: &str = "/cache/rf_pair_request"; // one-shot flag: open a pairing window

/// button-name -> IR action map, on the overlay. { "vol_up": {"device":"..","function":".."}, .. }
pub const MAP_PATH: &str = "/cache/remotemap.json";

pub const RFSPI: &str = "/dev/rfspi";
pub const RFFW: &str = "/dev/rffw";
pub const FW: &str = "/lib/firmware/cc2544.bin";

// opcodes we know from hal's cc2544 opcode->name table (char* array @0x461dc4, indexed by frame byte0)
#[allow(dead_code)]
pub const OP_LEDS: u8 = 0x11;
#[allow(dead_code)]
pub const OP_REPORTING: u8 = 0x12;
#[allow(dead_code)]
pub const OP_STATUS: u8 = 0x15;

// --- Verified host->radio commands (recovered from stock hal .data templates) -----------------
// hal writes these verbatim to /dev/rfspi (the driver clocks them out unchanged). Recovered by
// disassembling hal and cross-checked live: INIT matches the radio's idle status beacon
// `10 ff 80 00 00 00 00`, and the `10 ff 80 b2 <action>` pairing family sits next to the
// libhal_rf_host_{open_lock,close_lock,unpair} name strings in .data (adversarially verified).
// Framing of the 0x10 family: 10 ff <state> <selector> <p0> <p1> <p2>; state 0x80=host-control,
// 0x83=query; selector 0xb2=lock/pair control with byte4 = sub-action.

/// Radio bring-up command hal issues at boot (rf_msg_init, template @0x44b98c). Idempotent.
pub const INIT: [u8; 7] = [0x10, 0xff, 0x80, 0x00, 0x00, 0x01, 0x00];
/// Close the pairing/discovery window (libhal_rf_host_close_lock, template @0x44aef0). The window
/// also auto-closes after the `pair_open` seconds byte, so this is only needed to close early.
#[allow(dead_code)]
pub const PAIR_CLOSE: [u8; 7] = [0x10, 0xff, 0x80, 0xb2, 0x02, 0x00, 0x00];

/// Open the pairing/discovery window for `secs` seconds (libhal_rf_host_open_lock, byte6=window;
/// hal uses 0x5a = 90). This is the software replacement for the stock "hold hub reset 30s".
pub fn pair_open(secs: u8) -> [u8; 7] {
    [0x10, 0xff, 0x80, 0xb2, 0x01, 0x00, secs]
}
/// Drop a paired device by slot index (libhal_rf_host_unpair, byte5=index).
#[allow(dead_code)]
pub fn unpair(idx: u8) -> [u8; 7] {
    [0x10, 0xff, 0x80, 0xb2, 0x03, idx, 0x00]
}
/// Query a paired-device slot; the reply carries the device id/info (libhal_get_paired_device_info).
#[allow(dead_code)]
pub fn paired_query(idx: u8) -> [u8; 7] {
    [0x10, 0xff, 0x83, 0xb5, idx, 0x00, 0x00]
}
/// REPORTING-enable (byte0=0x12, byte2=0x01, byte1=paired device id). NOTE: the only hal emitter
/// of 0x12 is the *external USB-dongle* path — for the built-in cc2544 the report gate is a paired
/// device + an internal subscription, so INIT alone may already make reports flow. Kept for the
/// live experiment (send by hand via `--cmd`); byte1 is unknown until we read a paired-device id.
#[allow(dead_code)]
pub fn reporting_enable(devid: u8) -> [u8; 10] {
    [0x12, devid, 0x01, 0, 0, 0, 0, 0, 0, 0]
}

/// Bring the radio up by sending the INIT command (same one hal issues at boot).
pub fn start(f: &mut File) -> Result<(), String> {
    send(f, &INIT)
}

// --- Harmony remote nRF24 AIR protocol (Harmoino project + HID) — INFORMATIONAL ONLY ----------
// This describes the radio<->remote OVER-THE-AIR link, which the cc2544 firmware handles internally
// and which we never see on /dev/rfspi. The remote is an nRF24LE1 (Nordic 8051+radio); the air link
// is UNENCRYPTED: 2 Mbps, 40-bit address, CRC-16, hops 12 channels; on-air a button press carries a
// little-endian u32 HID `command_id` at air-payload offset 2 (low byte 0xC1=keyboard,0xC3=consumer).
//
// The /dev/rfspi HOST framing for a button report was cracked live (see decode_command_id): a 0x20
// container `20 01 <page> <b3> <b4> …`, page 0x01=keyboard (usage@b4)/0x03=consumer (usage@b3). The
// reconstructed command_id matches the Harmoino BUTTONS command_ids below (confirmed: vol_up 0xE9C3,
// mute 0xE2C3, menu 0x006500C1) — so those command_ids are correct; only the earlier offset-2 scan
// was wrong. RF_CHANNELS/PAIR_ADDRESS describe the air link and are unused (informational).
#[allow(dead_code)]
pub const RF_CHANNELS: [u8; 12] = [5, 8, 14, 17, 32, 35, 41, 44, 62, 65, 71, 74];
#[allow(dead_code)]
pub const PAIR_ADDRESS: u64 = 0xBB0A_DCA5_75;

/// button command_id -> our short name (the Smart Control button set). command_id is the
/// little-endian u32 at payload offset 2. Values from Harmoino, matched to this remote's buttons.
pub const BUTTONS: &[(u32, &str)] = &[
    (0x0058_00C1, "ok"), (0x0052_00C1, "up"), (0x0051_00C1, "down"),
    (0x0050_00C1, "left"), (0x004F_00C1, "right"), (0x0065_00C1, "menu"),
    (0x0056_00C1, "prev"), (0x0028_00C1, "enter"),
    (0x001E_00C1, "1"), (0x001F_00C1, "2"), (0x0020_00C1, "3"), (0x0021_00C1, "4"),
    (0x0022_00C1, "5"), (0x0023_00C1, "6"), (0x0024_00C1, "7"), (0x0025_00C1, "8"),
    (0x0026_00C1, "9"), (0x0027_00C1, "0"),
    (0x0000_E9C3, "vol_up"), (0x0000_EAC3, "vol_down"), (0x0000_9CC3, "ch_up"),
    (0x0000_9DC3, "ch_down"), (0x0000_E2C3, "mute"), (0x0002_24C3, "back"),
    (0x0000_94C3, "exit"), (0x0000_9AC3, "dvr"), (0x0000_8DC3, "guide"), (0x0001_FFC3, "info"),
    (0x0001_F7C3, "red"), (0x0001_F6C3, "green"), (0x0001_F5C3, "yellow"), (0x0001_F4C3, "blue"),
    (0x0000_B4C3, "rewind"), (0x0000_B3C3, "forward"), (0x0000_B0C3, "play"),
    (0x0000_B1C3, "pause"), (0x0000_B7C3, "stop"), (0x0000_B2C3, "record"),
    (0x0001_E8C3, "music"), (0x0001_EDC3, "tv"), (0x0001_E9C3, "movie"), (0x0001_ECC3, "off"),
];

pub fn button_name(code: u32) -> Option<&'static str> {
    BUTTONS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// One button-press action. `kind`:
///   "ir"      → fire an IR-DB code (device/function)
///   "ha"      → surface to Home Assistant only (no local IR)
///   "profile" → switch the active profile to `profile`
///   "none"    → unmapped
#[derive(Clone, Default)]
pub struct Action {
    pub kind: String,
    pub device: String,
    pub function: String,
    pub profile: String,
}

impl Action {
    fn from_value(v: &Value) -> Action {
        Action {
            kind: v.get("type").and_then(|x| x.as_str()).unwrap_or("none").to_string(),
            device: v.get("device").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            function: v.get("function").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            profile: v.get("profile").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        }
    }
    fn to_value(&self) -> Value {
        Value::Obj(vec![
            ("type".into(), Value::str(self.kind.as_str())),
            ("device".into(), Value::str(self.device.as_str())),
            ("function".into(), Value::str(self.function.as_str())),
            ("profile".into(), Value::str(self.profile.as_str())),
        ])
    }
    pub fn is_some(&self) -> bool {
        !self.kind.is_empty() && self.kind != "none"
    }
}

// --- helpers for reading/mutating a Value::Obj's Vec<(String, Value)> --------------------------
fn obj_get<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}
fn obj_get_mut<'a>(pairs: &'a mut Vec<(String, Value)>, key: &str) -> Option<&'a mut Value> {
    pairs.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
}
fn obj_set(pairs: &mut Vec<(String, Value)>, key: &str, val: Value) {
    match pairs.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = val,
        None => pairs.push((key.to_string(), val)),
    }
}

/// Read remotemap.json as the v2 root {active, profiles:{name:{button:{short?,long?}}}}. Migrates a
/// v1 map ({button:{type,device,function}}) into profiles.default[button].short; missing/invalid →
/// a fresh {active:"default", profiles:{default:{}}}.
fn read_root() -> Value {
    let parsed = std::fs::read_to_string(MAP_PATH).ok().and_then(|t| json::parse(&t).ok());
    if let Some(Value::Obj(pairs)) = parsed {
        if obj_get(&pairs, "profiles").is_some() {
            return Value::Obj(pairs); // already v2
        }
        // v1 → v2: wrap each button's action as its "short" action under profiles.default
        let buttons: Vec<(String, Value)> = pairs
            .into_iter()
            .map(|(btn, act)| (btn, Value::Obj(vec![("short".into(), act)])))
            .collect();
        return Value::Obj(vec![
            ("active".into(), Value::str("default")),
            ("profiles".into(), Value::Obj(vec![("default".into(), Value::Obj(buttons))])),
        ]);
    }
    Value::Obj(vec![
        ("active".into(), Value::str("default")),
        ("profiles".into(), Value::Obj(vec![("default".into(), Value::Obj(vec![]))])),
    ])
}

fn write_root(root: &Value) -> Result<(), String> {
    let tmp = format!("{}.tmp", MAP_PATH);
    std::fs::write(&tmp, root.to_string()).map_err(|e| format!("write {}: {}", tmp, e))?;
    std::fs::rename(&tmp, MAP_PATH).map_err(|e| format!("rename map: {}", e))?;
    Ok(())
}

/// Ensure profiles[name] exists (as an empty button map).
fn ensure_profile(rootpairs: &mut Vec<(String, Value)>, name: &str) {
    if obj_get_mut(rootpairs, "profiles").is_none() {
        obj_set(rootpairs, "profiles", Value::Obj(vec![]));
    }
    if let Some(Value::Obj(profs)) = obj_get_mut(rootpairs, "profiles") {
        if obj_get(profs, name).is_none() {
            profs.push((name.to_string(), Value::Obj(vec![])));
        }
    }
}

/// The active profile name.
pub fn active_profile() -> String {
    if let Value::Obj(pairs) = read_root() {
        if let Some(a) = obj_get(&pairs, "active").and_then(|x| x.as_str()) {
            return a.to_string();
        }
    }
    "default".to_string()
}

/// The full map as a v2 JSON string (migrating a v1 file on the fly) — for GET /api/rf/map so the
/// web UI always sees {active, profiles:{...}} even before the first edit persists v2.
pub fn map_json() -> String {
    read_root().to_string()
}

/// (active, all profile names).
pub fn list_profiles() -> (String, Vec<String>) {
    let mut names = Vec::new();
    let mut active = "default".to_string();
    if let Value::Obj(pairs) = read_root() {
        if let Some(a) = obj_get(&pairs, "active").and_then(|x| x.as_str()) {
            active = a.to_string();
        }
        if let Some(Value::Obj(profs)) = obj_get(&pairs, "profiles").map(|v| v.clone()) {
            for (n, _) in profs {
                names.push(n);
            }
        }
    }
    if names.is_empty() {
        names.push("default".into());
    }
    (active, names)
}

/// A button's action in the active profile (long=true → the long-press action).
pub fn button_action(button: &str, long: bool) -> Option<Action> {
    let root = read_root();
    let pairs = if let Value::Obj(p) = &root { p } else { return None };
    let active = obj_get(pairs, "active").and_then(|x| x.as_str()).unwrap_or("default");
    let entry = obj_get(pairs, "profiles")?.get(active)?.get(button)?;
    let a = Action::from_value(entry.get(if long { "long" } else { "short" })?);
    if a.is_some() { Some(a) } else { None }
}

/// Number of buttons mapped in the active profile (for logging).
pub fn active_mapping_count() -> usize {
    let root = read_root();
    if let Value::Obj(pairs) = &root {
        let active = obj_get(pairs, "active").and_then(|x| x.as_str()).unwrap_or("default");
        if let Some(v) = obj_get(pairs, "profiles").and_then(|p| p.get(active)) {
            if let Value::Obj(b) = v {
                return b.len();
            }
        }
    }
    0
}

/// Switch the active profile (creating it if missing).
pub fn set_active(name: &str) -> Result<(), String> {
    let mut root = read_root();
    if let Value::Obj(ref mut pairs) = root {
        ensure_profile(pairs, name);
        obj_set(pairs, "active", Value::str(name));
    }
    write_root(&root)
}

/// Create an (empty) profile.
pub fn create_profile(name: &str) -> Result<(), String> {
    let mut root = read_root();
    if let Value::Obj(ref mut pairs) = root {
        ensure_profile(pairs, name);
    }
    write_root(&root)
}

/// Delete a profile (never the last one; if it was active, fall back to another).
pub fn delete_profile(name: &str) -> Result<(), String> {
    let mut root = read_root();
    let mut new_active: Option<String> = None;
    if let Value::Obj(ref mut pairs) = root {
        let active = obj_get(pairs, "active").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if let Some(Value::Obj(profs)) = obj_get_mut(pairs, "profiles") {
            if profs.len() <= 1 {
                return Err("cannot delete the last profile".into());
            }
            profs.retain(|(k, _)| k != name);
            if active == name {
                new_active = profs.first().map(|(k, _)| k.clone());
            }
        }
        if let Some(a) = new_active {
            obj_set(pairs, "active", Value::str(a.as_str()));
        }
    }
    write_root(&root)
}

/// Upsert one button's short/long action within a profile (kind "none" clears that press).
pub fn save_action(profile: &str, button: &str, press: &str, action: &Action) -> Result<(), String> {
    let mut root = read_root();
    if let Value::Obj(ref mut pairs) = root {
        ensure_profile(pairs, profile);
        if let Some(Value::Obj(profs)) = obj_get_mut(pairs, "profiles") {
            if let Some(Value::Obj(btns)) = obj_get_mut(profs, profile) {
                if obj_get_mut(btns, button).is_none() {
                    btns.push((button.to_string(), Value::Obj(vec![])));
                }
                if let Some(Value::Obj(entry)) = obj_get_mut(btns, button) {
                    if action.is_some() {
                        obj_set(entry, press, action.to_value());
                    } else {
                        entry.retain(|(k, _)| k != press);
                    }
                }
                // drop the button entry if it has neither short nor long left
                let drop = obj_get(btns, button)
                    .and_then(|v| if let Value::Obj(e) = v { Some(e.is_empty()) } else { None })
                    .unwrap_or(false);
                if drop {
                    btns.retain(|(k, _)| k != button);
                }
            }
        }
    }
    write_root(&root)
}

/// Write the listener's pid so /api/rf/pair can stop it to open a pairing window.
pub fn write_pid() {
    let _ = std::fs::write(PID_PATH, unsafe { getpid() }.to_string());
}

/// One-shot: true (and clears the flag) if the web UI requested a pairing window.
pub fn take_pair_request() -> bool {
    if std::path::Path::new(PAIR_REQ_PATH).exists() {
        let _ = std::fs::remove_file(PAIR_REQ_PATH);
        true
    } else {
        false
    }
}

/// Highest press seq recorded so far (keeps the counter monotonic across listener restarts).
pub fn last_seq() -> u64 {
    std::fs::read_to_string(LAST_PATH)
        .ok()
        .and_then(|t| json::parse(&t).ok())
        .and_then(|v| v.get("seq").and_then(|s| s.as_i64()))
        .unwrap_or(0)
        .max(0) as u64
}

/// Record a decoded press for the web UI live feed + Home Assistant (atomic write). Clients poll
/// this and use `seq` to detect a new press. `long`=a long-press; `profile`=the active profile.
pub fn record_press(seq: u64, button: &str, code: u32, action: &str, long: bool, profile: &str) {
    let v = Value::Obj(vec![
        ("seq".into(), Value::int(seq as i64)),
        ("button".into(), Value::str(button)),
        ("id".into(), Value::str(format!("{:#010x}", code))),
        ("action".into(), Value::str(action)),
        ("hold".into(), Value::Bool(long)),
        ("profile".into(), Value::str(profile)),
    ]);
    let tmp = format!("{}.tmp", LAST_PATH);
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, LAST_PATH);
    }
}

/// True if the frame is a key-UP (a 0x20 report on either page with usage 0). The remote sends this
/// when a held button is released — used to time short vs long presses.
pub fn is_release(pkt: &[u8]) -> bool {
    pkt.len() >= 5 && pkt[0] == 0x20 && pkt[1] == 0x01 && (pkt[2] == 0x01 || pkt[2] == 0x03) && pkt[3] == 0 && pkt[4] == 0
}

/// Decode the HID command_id from a button-report frame. **Confirmed live** on the paired remote:
/// a report is a 0x20 container `20 01 <page> <b3> <b4> … <counter>`, where
///   page (byte2) = 0x01 keyboard  → usage16 = (b3<<8)|b4, command_id = (usage16<<16) | 0x00C1
///   page (byte2) = 0x03 consumer  → usage16 = (b4<<8)|b3, command_id = (usage16<<8)  | 0x00C3
/// usage 0 = key release; page 0x41 = link status, 0x42 = idle heartbeat (not buttons). The
/// resulting command_id matches the Harmoino BUTTONS table (e.g. vol_up 0xE9C3, menu 0x006500C1).
pub fn decode_command_id(pkt: &[u8]) -> Option<u32> {
    if pkt.len() < 5 || pkt[0] != 0x20 || pkt[1] != 0x01 {
        return None;
    }
    match pkt[2] {
        0x01 => {
            let usage = ((pkt[3] as u32) << 8) | pkt[4] as u32;
            if usage == 0 { None } else { Some((usage << 16) | 0x0000_00C1) }
        }
        0x03 => {
            let usage = ((pkt[4] as u32) << 8) | pkt[3] as u32;
            if usage == 0 { None } else { Some((usage << 8) | 0x0000_00C3) }
        }
        _ => None,
    }
}

/// Decode + name a button from a report frame (None if not a button or not in the BUTTONS table).
pub fn decode_button(pkt: &[u8]) -> Option<(u32, &'static str)> {
    decode_command_id(pkt).and_then(|c| button_name(c).map(|n| (c, n)))
}

// MIPS-specific flag values (this binary targets big-endian MIPS): O_NONBLOCK is 0x80 on MIPS
// (not 0x800 as on most arches). The cc2544 driver has no usable .poll, so we open non-blocking
// and time out with a sleep loop instead.
const O_NONBLOCK: i32 = 0x80;

/// Load the cc2544 firmware into the chip (idempotent; rcS.local already does this at boot).
pub fn load_fw() -> Result<(), String> {
    if !std::path::Path::new(RFFW).exists() {
        return Err(format!("{} missing — is cc2544.ko loaded?", RFFW));
    }
    let data = std::fs::read(FW).map_err(|e| format!("read {}: {}", FW, e))?;
    let mut f = OpenOptions::new().write(true).open(RFFW).map_err(|e| format!("open {}: {}", RFFW, e))?;
    f.write_all(&data).map_err(|e| format!("write fw: {}", e))?;
    Ok(())
}

/// Open the radio command/report channel, non-blocking (O_RDWR|O_NONBLOCK).
pub fn open_rf() -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(RFSPI)
        .map_err(|e| format!("open {}: {} (fw loaded? cc2544.ko present?)", RFSPI, e))
}

/// Send a raw command to the radio: write({opcode, args…}). ONE write() delivers the whole command:
/// the driver's write fop (cc2544_spi_write) copy_from_user's the entire buffer, then
/// cc2544_spi_readwrite bit-bangs it over the shared-NOR SPI bus and returns the RESPONSE byte
/// count — which is 0 for a fire-and-forget command. So Ok(0) is SUCCESS, not a short write (this is
/// exactly what stock hal's rf_msg_write assumes — it only treats a NEGATIVE return as an error).
/// O_NONBLOCK does not affect this path (the driver ignores it for writes). Call from a DETACHED
/// process: the bus grab can briefly block if the NOR flash is busy.
pub fn send(f: &mut File, cmd: &[u8]) -> Result<(), String> {
    match f.write(cmd) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("write rfspi: {}", e)),
    }
}

/// Read exactly one packet, BLOCKING. The cc2544 driver's read fop (cc2544_spi_read) ignores
/// O_NONBLOCK and sleeps on an empty queue (prepare_to_wait/schedule), only waking on a new packet
/// or a signal — there is NO non-blocking read. A reader must therefore block until a packet
/// arrives; bound a capture with `arm_timeout()` (SIGALRM) and/or an external kill. packet[0]=type.
pub fn recv_blocking(f: &File) -> Result<Vec<u8>, String> {
    let mut buf = [0u8; 256];
    loop {
        match (&*f).read(&mut buf) {
            Ok(0) => return Err("rfspi read returned 0".into()),
            Ok(n) => return Ok(buf[..n].to_vec()),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read rfspi: {}", e)),
        }
    }
}

/// Arm a hard self-terminate backstop: after `secs`, SIGALRM (default action = terminate) fires and
/// ends any blocked read, so a detached capture can't hang forever. secs=0 disables. Flush output
/// per line so nothing buffered is lost when it fires. (External `kill` gives precise early stop.)
pub fn arm_timeout(secs: u32) {
    unsafe { alarm(secs) };
}

/// Hex-dump a packet, naming the button when the 0x20 report decodes to a known command_id (and
/// still showing the raw command_id for an unmapped one, so new buttons can be captured/added).
pub fn describe(pkt: &[u8]) -> String {
    let hex: String = pkt.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
    let kind = if let Some((code, name)) = decode_button(pkt) {
        format!("BUTTON {:<9} (id {:#010x})", name, code)
    } else if let Some(code) = decode_command_id(pkt) {
        format!("report UNMAPPED (id {:#010x})", code)
    } else {
        match pkt.first().copied() {
            Some(0x10) => {
                let st = pkt.get(2).copied().unwrap_or(0);
                let s = if st & 1 != 0 { "remote-present" } else { "idle" };
                format!("STATUS/beacon state=0x{:02x} {}", st, s)
            }
            Some(0x20) => "STATUS/container".into(),
            Some(0x15) => "STATUS".into(),
            Some(0x22) => "ACK".into(),
            Some(t) => format!("type=0x{:02x}", t),
            None => "empty".into(),
        }
    };
    format!("[{:2}B] {:<32} {}", pkt.len(), kind, hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_real_report_frames() {
        // Real 0x20 container frames captured from the paired remote (padding trimmed).
        let volup = [0x20, 0x01, 0x03, 0xe9, 0x00, 0x00]; // consumer usage 0xe9 at byte3
        assert_eq!(decode_button(&volup), Some((0x0000_E9C3, "vol_up")));
        let mute = [0x20, 0x01, 0x03, 0xe2, 0x00, 0x00];
        assert_eq!(decode_button(&mute), Some((0x0000_E2C3, "mute")));
        let menu = [0x20, 0x01, 0x01, 0x00, 0x65, 0x00]; // keyboard usage 0x65 at byte4
        assert_eq!(decode_button(&menu), Some((0x0065_00C1, "menu")));
        // a 2-byte consumer usage (info = 0x1FFC3): b3=0xff low, b4=0x01 high
        assert_eq!(decode_command_id(&[0x20, 0x01, 0x03, 0xff, 0x01, 0x00]), Some(0x0001_FFC3));
        // release (usage 0) and status/heartbeat frames are not buttons
        assert!(decode_button(&[0x20, 0x01, 0x03, 0x00, 0x00, 0x00]).is_none());
        assert!(decode_button(&[0x20, 0x01, 0x42, 0x00, 0x00, 0x00]).is_none());
        assert!(decode_button(&[0x10, 0xff, 0x80, 0x00, 0x00, 0x00, 0x00]).is_none());
        assert_eq!(button_name(0x0001_ECC3), Some("off"));
    }
}

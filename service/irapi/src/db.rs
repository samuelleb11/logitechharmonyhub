//! Offline IR code database. Bundles a curated Flipper-IRDB subset (CC0) built by
//! tools/build_irdb.py into codes/irdb.txt (loaded from /cache/irdb.txt at runtime), plus
//! LEARNED custom codes on the overlay (/cache/custom.json). Codes are stored compactly as
//! protocol params (`P`, expanded on-device via `enc`) or raw µs (`R`). Browsable as
//! type -> brand -> device -> function; every code fires through i2s::blast.

use crate::enc::{self, Encoded};
use crate::json::{self, Value};
use std::sync::{Mutex, OnceLock};

const CUSTOM_PATH: &str = "/cache/custom.json";

enum FnCode {
    Raw { carrier: u32, duty: u8, us: Vec<u32> },
    Parsed { protocol: String, addr: Vec<u8>, cmd: Vec<u8> },
}

struct Function {
    name: String,
    code: FnCode,
}

struct Device {
    id: String,
    dtype: String,
    brand: String,
    model: String,
    fns: Vec<Function>,
}

pub struct DeviceInfo {
    pub id: String,
    pub dtype: String,
    pub brand: String,
    pub model: String,
}

static DB: OnceLock<Mutex<Vec<Device>>> = OnceLock::new();

fn db() -> &'static Mutex<Vec<Device>> {
    DB.get_or_init(|| Mutex::new(load_all()))
}

fn load_all() -> Vec<Device> {
    let mut v = parse_bundled(&load_text());
    v.extend(parse_custom());
    v
}

/// The bundled DB is an external file (deployed to /cache — the 5MB partition — so the binary
/// stays small). Falls back to /root then the embedded Danby default.
fn load_text() -> String {
    #[cfg(test)]
    {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/codes/irdb.txt"))
            .expect("test: codes/irdb.txt")
    }
    #[cfg(not(test))]
    {
        std::fs::read_to_string("/cache/irdb.txt")
            .or_else(|_| std::fs::read_to_string("/root/irdb.txt"))
            .unwrap_or_else(|_| include_str!("../codes/irdb_default.txt").to_string())
    }
}

fn parse_bundled(text: &str) -> Vec<Device> {
    let mut devs: Vec<Device> = Vec::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        match it.next() {
            Some("D") => devs.push(Device {
                id: it.next().unwrap_or("").to_string(),
                dtype: it.next().unwrap_or("").to_string(),
                brand: it.next().unwrap_or("").to_string(),
                model: it.next().unwrap_or("").to_string(),
                fns: Vec::new(),
            }),
            Some("F") => {
                let dev = match devs.last_mut() {
                    Some(d) => d,
                    None => continue,
                };
                let name = it.next().unwrap_or("").to_string();
                let code = match it.next() {
                    Some("P") => FnCode::Parsed {
                        protocol: it.next().unwrap_or("").to_string(),
                        addr: enc::hex_bytes(it.next().unwrap_or("")),
                        cmd: enc::hex_bytes(it.next().unwrap_or("")),
                    },
                    Some("R") => {
                        let carrier = it.next().and_then(|x| x.parse().ok()).unwrap_or(38000);
                        let duty = it.next().and_then(|x| x.parse().ok()).unwrap_or(33);
                        let us: Vec<u32> =
                            it.next().unwrap_or("").split(',').filter_map(|x| x.parse().ok()).collect();
                        if us.is_empty() {
                            continue;
                        }
                        FnCode::Raw { carrier, duty, us }
                    }
                    _ => continue,
                };
                dev.fns.push(Function { name, code });
            }
            _ => {}
        }
    }
    devs
}

/// Learned custom devices, from /cache/custom.json (JSON). Empty if missing/malformed.
fn parse_custom() -> Vec<Device> {
    let text = match std::fs::read_to_string(CUSTOM_PATH) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let root = match json::parse(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match root.get("devices").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    arr.iter()
        .map(|d| {
            let fns = d
                .get("functions")
                .and_then(|v| v.as_array())
                .map(|fa| {
                    fa.iter()
                        .filter_map(|f| {
                            let name = f.get("name").and_then(|x| x.as_str())?.to_string();
                            let carrier = f.get("carrier").and_then(|x| x.as_i64()).unwrap_or(38000) as u32;
                            let duty = f.get("duty").and_then(|x| x.as_i64()).unwrap_or(33) as u8;
                            let us: Vec<u32> = f
                                .get("us")
                                .and_then(|x| x.as_array())?
                                .iter()
                                .filter_map(|n| n.as_i64().map(|v| v.max(0) as u32))
                                .collect();
                            if us.is_empty() {
                                return None;
                            }
                            Some(Function { name, code: FnCode::Raw { carrier, duty, us } })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Device {
                id: s(d, "id"),
                dtype: s(d, "type"),
                brand: s(d, "brand"),
                model: s(d, "model"),
                fns,
            }
        })
        .collect()
}

fn uniq(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for x in items {
        if !v.iter().any(|e| e == &x) {
            v.push(x);
        }
    }
    v
}

pub fn types() -> Vec<String> {
    let g = db().lock().unwrap();
    uniq(g.iter().map(|d| d.dtype.clone()))
}

pub fn brands(dtype: &str) -> Vec<String> {
    let g = db().lock().unwrap();
    uniq(g.iter().filter(|d| d.dtype == dtype).map(|d| d.brand.clone()))
}

pub fn devices(dtype: Option<&str>, brand: Option<&str>) -> Vec<DeviceInfo> {
    let g = db().lock().unwrap();
    g.iter()
        .filter(|d| dtype.map_or(true, |t| d.dtype == t) && brand.map_or(true, |b| d.brand == b))
        .map(|d| DeviceInfo {
            id: d.id.clone(),
            dtype: d.dtype.clone(),
            brand: d.brand.clone(),
            model: d.model.clone(),
        })
        .collect()
}

pub fn functions(device_id: &str) -> Vec<String> {
    let g = db().lock().unwrap();
    g.iter()
        .find(|d| d.id == device_id)
        .map(|d| d.fns.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default()
}

/// Look up + expand a code to raw µs. Parsed codes are encoded on demand.
pub fn lookup(device_id: &str, function: &str) -> Option<Encoded> {
    let g = db().lock().unwrap();
    let f = g.iter().find(|d| d.id == device_id)?.fns.iter().find(|f| f.name == function)?;
    match &f.code {
        FnCode::Raw { carrier, duty, us } => {
            Some(Encoded { carrier: *carrier, duty: *duty, us: us.clone() })
        }
        FnCode::Parsed { protocol, addr, cmd } => enc::encode(protocol, addr, cmd),
    }
}

pub fn device_count() -> usize {
    db().lock().unwrap().len()
}

/// Reload the whole DB from disk (bundled /cache/irdb.txt + custom.json). After an OTA DB update.
pub fn reload() {
    *db().lock().unwrap() = load_all();
}

// ---------------------------------------------------------------------------
// Learned-code storage (custom.json on the /cache overlay)
// ---------------------------------------------------------------------------

/// Upsert a learned raw code into /cache/custom.json, then hot-reload the DB so it is
/// immediately browseable/fireable. `us` is alternating mark,space µs starting with a mark.
pub fn save_learned(
    dtype: &str,
    brand: &str,
    model: &str,
    device_id: &str,
    function: &str,
    carrier: u32,
    us: &[u32],
) -> Result<(), String> {
    // Read existing custom devices as Values, upsert, write back.
    let existing = std::fs::read_to_string(CUSTOM_PATH).ok();
    let mut devices: Vec<Value> = existing
        .as_deref()
        .and_then(|t| json::parse(t).ok())
        .and_then(|v| v.get("devices").and_then(|d| d.as_array()).cloned())
        .unwrap_or_default();

    let func = Value::Obj(vec![
        ("name".into(), Value::str(function)),
        ("carrier".into(), Value::int(carrier as i64)),
        ("duty".into(), Value::int(33)),
        ("us".into(), Value::Arr(us.iter().map(|&u| Value::int(u as i64)).collect())),
    ]);

    // find device by id
    let mut placed = false;
    for d in devices.iter_mut() {
        if d.get("id").and_then(|x| x.as_str()) == Some(device_id) {
            let mut fns = d.get("functions").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            fns.retain(|f| f.get("name").and_then(|x| x.as_str()) != Some(function));
            fns.push(func.clone());
            if let Value::Obj(pairs) = d {
                pairs.retain(|(k, _)| k != "functions");
                pairs.push(("functions".into(), Value::Arr(fns)));
            }
            placed = true;
            break;
        }
    }
    if !placed {
        devices.push(Value::Obj(vec![
            ("id".into(), Value::str(device_id)),
            ("type".into(), Value::str(dtype)),
            ("brand".into(), Value::str(brand)),
            ("model".into(), Value::str(model)),
            ("functions".into(), Value::Arr(vec![func])),
        ]));
    }

    let root = Value::Obj(vec![("devices".into(), Value::Arr(devices))]);
    let tmp = format!("{}.tmp", CUSTOM_PATH);
    std::fs::write(&tmp, root.to_string()).map_err(|e| format!("write {}: {}", tmp, e))?;
    std::fs::rename(&tmp, CUSTOM_PATH).map_err(|e| format!("rename custom.json: {}", e))?;

    // hot reload
    *db().lock().unwrap() = load_all();
    Ok(())
}

/// Remove a learned code from /cache/custom.json, then hot-reload. If `function` is empty the
/// whole custom device is removed; otherwise just that function (and the device too if it becomes
/// empty). Returns Ok(true) if something was removed, Ok(false) if nothing matched. Only affects
/// custom (learned) devices — the bundled DB is read-only.
pub fn forget_learned(device_id: &str, function: &str) -> Result<bool, String> {
    let text = match std::fs::read_to_string(CUSTOM_PATH) {
        Ok(t) => t,
        Err(_) => return Ok(false), // no custom store yet
    };
    let mut devices: Vec<Value> = json::parse(&text)
        .ok()
        .and_then(|v| v.get("devices").and_then(|d| d.as_array()).cloned())
        .unwrap_or_default();

    let before = devices.len();
    let mut changed = false;
    if function.is_empty() {
        devices.retain(|d| d.get("id").and_then(|x| x.as_str()) != Some(device_id));
        changed = devices.len() != before;
    } else {
        for d in devices.iter_mut() {
            if d.get("id").and_then(|x| x.as_str()) != Some(device_id) {
                continue;
            }
            let mut fns = d.get("functions").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let n = fns.len();
            fns.retain(|f| f.get("name").and_then(|x| x.as_str()) != Some(function));
            if fns.len() != n {
                changed = true;
            }
            if let Value::Obj(pairs) = d {
                pairs.retain(|(k, _)| k != "functions");
                pairs.push(("functions".into(), Value::Arr(fns)));
            }
        }
        // drop any device left with no functions
        devices.retain(|d| {
            d.get("functions").and_then(|x| x.as_array()).map_or(true, |a| !a.is_empty())
        });
    }

    if !changed {
        return Ok(false);
    }
    let root = Value::Obj(vec![("devices".into(), Value::Arr(devices))]);
    let tmp = format!("{}.tmp", CUSTOM_PATH);
    std::fs::write(&tmp, root.to_string()).map_err(|e| format!("write {}: {}", tmp, e))?;
    std::fs::rename(&tmp, CUSTOM_PATH).map_err(|e| format!("rename custom.json: {}", e))?;
    *db().lock().unwrap() = load_all();
    Ok(true)
}

/// Allocate an unused "custom-NNNN" device id.
pub fn next_custom_id() -> String {
    let g = db().lock().unwrap();
    for n in 1..10000 {
        let id = format!("custom-{:04}", n);
        if !g.iter().any(|d| d.id == id) {
            return id;
        }
    }
    "custom-0000".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn db_parses_and_browses() {
        assert!(device_count() >= 20, "expected a bundled DB, got {}", device_count());
        assert!(types().iter().any(|t| t == "TVs"));
        assert!(types().iter().any(|t| t == "ACs"));
        let danby = devices(Some("ACs"), Some("Danby"));
        assert_eq!(danby.len(), 1);
        let id = danby[0].id.clone();
        assert!(functions(&id).iter().any(|f| f == "Cool_hi"));
        let c = lookup(&id, "Cool_hi").expect("Cool_hi");
        assert_eq!(c.carrier, 38000);
        assert_eq!(c.seq()[0].0, true);
    }
}

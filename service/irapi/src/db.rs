//! Offline IR code database. Bundles a curated Flipper-IRDB subset (CC0 public domain) built
//! by tools/build_irdb.py into codes/irdb.txt, and parsed once into memory. Codes are stored
//! COMPACTLY as protocol params (`P`) and expanded to raw µs on lookup via `enc`, or as raw
//! µs (`R`) for captures (ACs). Browsable as type -> brand -> device -> function.

use crate::enc::{self, Encoded};
use std::sync::OnceLock;

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

static DB: OnceLock<Vec<Device>> = OnceLock::new();

fn db() -> &'static Vec<Device> {
    DB.get_or_init(|| parse(&load_text()))
}

/// The DB lives as an external file (deployed to /cache — a separate 5MB partition — so the
/// binary stays small and the catalog can grow/update independently). Falls back to /root,
/// then to the small embedded Danby default so the appliance always works.
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

fn parse(text: &str) -> Vec<Device> {
    let mut devs: Vec<Device> = Vec::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        match it.next() {
            Some("D") => {
                let id = it.next().unwrap_or("").to_string();
                let dtype = it.next().unwrap_or("").to_string();
                let brand = it.next().unwrap_or("").to_string();
                let model = it.next().unwrap_or("").to_string();
                devs.push(Device { id, dtype, brand, model, fns: Vec::new() });
            }
            Some("F") => {
                let dev = match devs.last_mut() {
                    Some(d) => d,
                    None => continue,
                };
                let name = it.next().unwrap_or("").to_string();
                let code = match it.next() {
                    Some("P") => {
                        let protocol = it.next().unwrap_or("").to_string();
                        let addr = enc::hex_bytes(it.next().unwrap_or(""));
                        let cmd = enc::hex_bytes(it.next().unwrap_or(""));
                        FnCode::Parsed { protocol, addr, cmd }
                    }
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

pub fn types() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for d in db() {
        if !v.iter().any(|t| t == &d.dtype) {
            v.push(d.dtype.clone());
        }
    }
    v
}

pub fn brands(dtype: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for d in db() {
        if d.dtype == dtype && !v.iter().any(|b| b == &d.brand) {
            v.push(d.brand.clone());
        }
    }
    v
}

pub fn devices(dtype: Option<&str>, brand: Option<&str>) -> Vec<DeviceInfo> {
    db()
        .iter()
        .filter(|d| dtype.map_or(true, |t| d.dtype == t) && brand.map_or(true, |b| d.brand == b))
        .map(|d| DeviceInfo {
            id: d.id.clone(),
            dtype: d.dtype.clone(),
            brand: d.brand.clone(),
            model: d.model.clone(),
        })
        .collect()
}

fn find(id: &str) -> Option<&'static Device> {
    db().iter().find(|d| d.id == id)
}

pub fn functions(device_id: &str) -> Vec<String> {
    find(device_id).map(|d| d.fns.iter().map(|f| f.name.clone()).collect()).unwrap_or_default()
}

/// Look up + expand a code to raw µs. Parsed codes are encoded on demand.
pub fn lookup(device_id: &str, function: &str) -> Option<Encoded> {
    let f = find(device_id)?.fns.iter().find(|f| f.name == function)?;
    match &f.code {
        FnCode::Raw { carrier, duty, us } => {
            Some(Encoded { carrier: *carrier, duty: *duty, us: us.clone() })
        }
        FnCode::Parsed { protocol, addr, cmd } => enc::encode(protocol, addr, cmd),
    }
}

pub fn device_count() -> usize {
    db().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn db_parses_and_browses() {
        assert!(device_count() >= 20, "expected a bundled DB, got {}", device_count());
        let ts = types();
        assert!(ts.iter().any(|t| t == "TVs"), "types: {:?}", ts);
        assert!(ts.iter().any(|t| t == "ACs"));
        // Danby AC still present and fires as raw.
        let danby = devices(Some("ACs"), Some("Danby"));
        assert_eq!(danby.len(), 1);
        let id = &danby[0].id;
        assert!(functions(id).iter().any(|f| f == "Cool_hi"));
        let c = lookup(id, "Cool_hi").expect("Cool_hi");
        assert_eq!(c.carrier, 38000);
        assert_eq!(c.seq()[0].0, true);
    }
    #[test]
    fn parsed_tv_code_encodes() {
        // A Samsung TV Power should exist and expand to a valid NEC-family frame.
        let s = devices(Some("TVs"), Some("Samsung"));
        assert!(!s.is_empty());
        let id = &s[0].id;
        let f = functions(id);
        if let Some(pw) = f.iter().find(|x| x.to_lowercase() == "power") {
            let e = lookup(id, pw).expect("power encodes");
            assert!(e.us.len() >= 60, "frame too short: {}", e.us.len());
        }
    }
}

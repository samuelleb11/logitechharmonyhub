//! Minimal single-threaded HTTP management server (the web UI). No threads (futex),
//! no external crates. Serves one self-contained page + a small JSON API.

use crate::ir::LtcpBackend;
use crate::json::{self, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

const INDEX: &str = include_str!("../web/index.html");

fn sh(cmd: &str) -> String {
    match Command::new("/bin/sh").arg("-c").arg(cmd).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Value between the first `after` and the next `end` in `s` (for parsing tool output).
fn between<'a>(s: &'a str, after: &str, end: &str) -> Option<&'a str> {
    let i = s.find(after)? + after.len();
    let j = s[i..].find(end)? + i;
    Some(&s[i..j])
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// Escape a value for inclusion inside a wpa_supplicant.conf quoted string; drop
/// control chars/newlines so an SSID/PSK can't inject extra config lines.
fn cfg_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o
}

struct Req {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_req(stream: &TcpStream) -> Option<Req> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut it = line.split_whitespace();
    let method = it.next()?.to_string();
    let path = it.next()?.to_string();
    let mut clen = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let t = h.trim_end();
        if t.is_empty() {
            break;
        }
        let l = t.to_ascii_lowercase();
        if let Some(v) = l.strip_prefix("content-length:") {
            clen = v.trim().parse().unwrap_or(0);
        }
    }
    let clen = clen.min(64 * 1024); // cap body
    let mut body = vec![0u8; clen];
    if clen > 0 && reader.read_exact(&mut body).is_err() {
        return None;
    }
    Some(Req { method, path, body })
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let hdr = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        status, ctype, body.len()
    );
    let _ = s.write_all(hdr.as_bytes());
    let _ = s.write_all(body);
}

fn json_resp(s: &mut TcpStream, status: &str, v: &Value) {
    respond(s, status, "application/json", v.to_string().as_bytes());
}

pub fn serve(port: u16, hal_addr: &str) -> i32 {
    let listener = match TcpListener::bind(format!("0.0.0.0:{}", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("serve: bind 0.0.0.0:{} failed: {}", port, e);
            return 1;
        }
    };
    eprintln!("[web] management UI on 0.0.0.0:{}", port);
    for c in listener.incoming() {
        let mut s = match c {
            Ok(s) => s,
            Err(_) => continue,
        };
        s.set_read_timeout(Some(std::time::Duration::from_secs(15))).ok();
        s.set_write_timeout(Some(std::time::Duration::from_secs(15))).ok();
        if let Some(req) = read_req(&s) {
            route(&mut s, &req, hal_addr);
        }
        // one connection to completion; Connection: close
    }
    0
}

fn route(s: &mut TcpStream, req: &Req, hal_addr: &str) {
    let (path, query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
    match (req.method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => {
            respond(s, "200 OK", "text/html; charset=utf-8", INDEX.as_bytes())
        }
        ("GET", "/api/status") | ("GET", "/api/health") => api_status(s, hal_addr),
        ("GET", "/api/wifi/scan") => api_scan(s),
        ("GET", "/api/ir/types") => api_ir_types(s),
        ("GET", "/api/ir/brands") => api_ir_brands(s, query),
        ("GET", "/api/ir/devices") => api_ir_devices(s, query),
        ("GET", "/api/ir/functions") => api_ir_functions(s, query),
        ("POST", "/api/ir/send") => api_ir_send(s, &req.body),
        ("GET", "/api/ha/rest_command.yaml") => api_ha_yaml(s),
        ("POST", "/api/wifi/connect") => api_wifi_connect(s, &req.body),
        ("POST", "/api/ir/test") => api_ir_test(s, hal_addr),
        ("POST", "/api/ir/loopback") => api_ir_loopback(s, hal_addr),
        ("POST", "/api/reboot") => {
            json_resp(s, "200 OK", &obj(vec![("ok", Value::Bool(true))]));
            sh("(sleep 1; reboot) &");
        }
        ("POST", "/api/factory-reset") => {
            json_resp(
                s,
                "200 OK",
                &obj(vec![
                    ("ok", Value::Bool(true)),
                    ("detail", Value::str("erased Wi-Fi + codes; rebooting into setup AP")),
                ]),
            );
            // Keep the appliance (rcS.local, irapi); drop user data -> AP fallback on reboot.
            sh("rm -f /etc/wifi/wpa_supplicant.conf /root/irdb.json; (sleep 2; reboot) &");
        }
        _ => respond(s, "404 Not Found", "text/plain", b"not found"),
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(n) => {
                    out.push(n as char);
                    i += 3;
                }
                Err(_) => {
                    out.push('%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(' ');
                i += 1;
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}

fn query_get(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        kv.split_once('=').and_then(|(k, v)| if k == key { Some(url_decode(v)) } else { None })
    })
}

/// GET /api/ir/types — device categories.
fn api_ir_types(s: &mut TcpStream) {
    let arr: Vec<Value> = crate::db::types().into_iter().map(Value::str).collect();
    json_resp(s, "200 OK", &obj(vec![("types", Value::Arr(arr))]));
}

/// GET /api/ir/brands?type=TVs — brands within a type.
fn api_ir_brands(s: &mut TcpStream, query: &str) {
    let t = query_get(query, "type").unwrap_or_default();
    let arr: Vec<Value> = crate::db::brands(&t).into_iter().map(Value::str).collect();
    json_resp(s, "200 OK", &obj(vec![("brands", Value::Arr(arr))]));
}

/// GET /api/ir/devices[?type=&brand=] — devices, optionally filtered.
fn api_ir_devices(s: &mut TcpStream, query: &str) {
    let t = query_get(query, "type");
    let b = query_get(query, "brand");
    let arr: Vec<Value> = crate::db::devices(t.as_deref(), b.as_deref())
        .into_iter()
        .map(|d| {
            obj(vec![
                ("id", Value::str(d.id)),
                ("type", Value::str(d.dtype)),
                ("brand", Value::str(d.brand)),
                ("model", Value::str(d.model)),
            ])
        })
        .collect();
    json_resp(s, "200 OK", &obj(vec![("devices", Value::Arr(arr))]));
}

/// GET /api/ha/rest_command.yaml — a copy-paste Home Assistant rest_command config that
/// fires any code by device+function through POST /api/ir/send.
fn api_ha_yaml(s: &mut TcpStream) {
    let ip = between(&sh("ifconfig ath0 2>/dev/null"), "inet addr:", " ")
        .filter(|s| !s.is_empty())
        .unwrap_or("<hub-ip>")
        .to_string();
    let yaml = format!(
        "# Home Assistant config — fire any Harmony-Hub IR code.\n\
         # In configuration.yaml, then call service rest_command.ir_send with data:\n\
         #   {{ \"device\": \"tv_samsung_...\", \"function\": \"Power\" }}\n\
         rest_command:\n\
         \x20 ir_send:\n\
         \x20   url: \"http://{}/api/ir/send\"\n\
         \x20   method: POST\n\
         \x20   content_type: \"application/json\"\n\
         \x20   payload: '{{\"device\":\"{{{{ device }}}}\",\"function\":\"{{{{ function }}}}\",\"select\":{{{{ select|default(7) }}}}}}'\n",
        ip
    );
    respond(s, "200 OK", "text/yaml; charset=utf-8", yaml.as_bytes());
}

/// GET /api/ir/functions?device=ID — list a device's function (button) names.
fn api_ir_functions(s: &mut TcpStream, query: &str) {
    let device = query_get(query, "device").unwrap_or_default();
    let fns: Vec<Value> = crate::db::functions(&device).into_iter().map(Value::str).collect();
    json_resp(s, "200 OK", &obj(vec![("functions", Value::Arr(fns))]));
}

/// POST /api/ir/send — fire a code via the direct I2S driver. Body is ONE of:
///   {"device":"..","function":".."[,"select":7]}   (DB lookup; "command" alias for HA)
///   {"raw_us":[..],"carrier":38000,"duty":33,"select":7}
fn api_ir_send(s: &mut TcpStream, body: &[u8]) {
    let v = json::parse(&String::from_utf8_lossy(body)).unwrap_or(Value::Null);
    let select = v.get("select").and_then(|x| x.as_i64()).unwrap_or(7) as u32;
    if let Some(arr) = v.get("raw_us").and_then(|x| x.as_array()) {
        let carrier = v.get("carrier").and_then(|x| x.as_i64()).unwrap_or(38000) as u32;
        let duty = v.get("duty").and_then(|x| x.as_i64()).unwrap_or(33) as u8;
        let seq: Vec<(bool, u32)> = arr
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_i64().map(|n| (i % 2 == 0, n.max(0) as u32)))
            .collect();
        if seq.is_empty() {
            return json_err(s, "400 Bad Request", "empty raw_us");
        }
        return blast_resp(s, carrier, duty, select, &seq);
    }
    let device = v.get("device").and_then(|x| x.as_str()).unwrap_or("");
    let func = v
        .get("function")
        .or_else(|| v.get("command"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if device.is_empty() || func.is_empty() {
        return json_err(s, "400 Bad Request", "need raw_us, or device+function");
    }
    match crate::db::lookup(device, func) {
        Some(r) => blast_resp(s, r.carrier, r.duty, select, &r.seq()),
        None => json_err(s, "404 Not Found", "no such device/function"),
    }
}

fn blast_resp(s: &mut TcpStream, carrier: u32, duty: u8, select: u32, seq: &[(bool, u32)]) {
    match crate::i2s::blast(carrier, duty.max(1), 1, select, seq, crate::i2s::Finish::Drain) {
        Ok(()) => json_resp(
            s,
            "200 OK",
            &obj(vec![("ok", Value::Bool(true)), ("emitted", Value::int(seq.len() as i64))]),
        ),
        Err(e) => json_err(s, "502 Bad Gateway", &e),
    }
}

fn json_err(s: &mut TcpStream, status: &str, msg: &str) {
    json_resp(s, status, &obj(vec![("ok", Value::Bool(false)), ("error", Value::str(msg))]));
}

fn api_status(s: &mut TcpStream, hal_addr: &str) {
    let iwc0 = sh("iwconfig ath0 2>/dev/null");
    let iwc1 = sh("iwconfig ath1 2>/dev/null");
    let sta_ssid = between(&iwc0, "ESSID:\"", "\"").unwrap_or("").to_string();
    let ap_ssid = between(&iwc1, "ESSID:\"", "\"").unwrap_or("").to_string();
    let ip0 = between(&sh("ifconfig ath0 2>/dev/null"), "inet addr:", " ").unwrap_or("").to_string();
    let ip1 = between(&sh("ifconfig ath1 2>/dev/null"), "inet addr:", " ").unwrap_or("").to_string();
    let (mode, ssid, ip) = if !ap_ssid.is_empty() {
        ("AP", ap_ssid, ip1)
    } else if !sta_ssid.is_empty() {
        ("station", sta_ssid, ip0)
    } else {
        ("down", String::new(), String::new())
    };
    // IR readiness = the direct-I2S transmit path is available (no hal dependency).
    let ir = if std::path::Path::new("/dev/i2s").exists() {
        "ready"
    } else {
        "no /dev/i2s"
    };
    let uptime = sh("cut -d. -f1 /proc/uptime").trim().to_string();
    let _ = hal_addr;
    json_resp(
        s,
        "200 OK",
        &obj(vec![
            ("mode", Value::str(mode)),
            ("ssid", Value::str(ssid)),
            ("ip", Value::str(ip)),
            ("ir", Value::str(ir)),
            ("uptime", Value::str(uptime)),
            ("version", Value::str(env!("CARGO_PKG_VERSION"))),
        ]),
    );
}

fn api_scan(s: &mut TcpStream) {
    // Best-effort: needs a station VAP (ath0). In AP-only mode there may be none, then the
    // user just types the SSID manually in the connect form.
    let out = sh("iwlist ath0 scan 2>/dev/null");
    let mut nets: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for cell in out.split("Cell ").skip(1) {
        let ssid = between(cell, "ESSID:\"", "\"").unwrap_or("").to_string();
        if ssid.is_empty() || seen.contains(&ssid) {
            continue;
        }
        seen.push(ssid.clone());
        let signal = between(cell, "Signal level=", " ")
            .or_else(|| between(cell, "Quality=", " "))
            .unwrap_or("")
            .to_string();
        let enc = cell.contains("Encryption key:on");
        nets.push(obj(vec![
            ("ssid", Value::str(ssid)),
            ("signal", Value::str(signal)),
            ("enc", Value::Bool(enc)),
        ]));
    }
    json_resp(s, "200 OK", &obj(vec![("networks", Value::Arr(nets))]));
}

fn api_wifi_connect(s: &mut TcpStream, body: &[u8]) {
    let v = json::parse(&String::from_utf8_lossy(body)).unwrap_or(Value::Null);
    let ssid = v.get("ssid").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let psk = v.get("psk").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if ssid.is_empty() {
        json_resp(
            s,
            "400 Bad Request",
            &obj(vec![("ok", Value::Bool(false)), ("error", Value::str("ssid required"))]),
        );
        return;
    }
    let conf = if psk.is_empty() {
        format!(
            "ctrl_interface=/var/run/wpa_supplicant\nupdate_config=1\nap_scan=1\n\nnetwork={{\n\tssid=\"{}\"\n\tkey_mgmt=NONE\n}}\n",
            cfg_escape(&ssid)
        )
    } else {
        format!(
            "ctrl_interface=/var/run/wpa_supplicant\nupdate_config=1\nap_scan=1\n\nnetwork={{\n\tssid=\"{}\"\n\tpsk=\"{}\"\n\tkey_mgmt=WPA-PSK\n\tproto=RSN WPA\n\tpairwise=CCMP TKIP\n}}\n",
            cfg_escape(&ssid),
            cfg_escape(&psk)
        )
    };
    if std::fs::write("/etc/wifi/wpa_supplicant.conf", conf).is_err() {
        json_resp(
            s,
            "500 Internal Server Error",
            &obj(vec![("ok", Value::Bool(false)), ("error", Value::str("write failed"))]),
        );
        return;
    }
    sh("chmod 600 /etc/wifi/wpa_supplicant.conf");
    json_resp(
        s,
        "200 OK",
        &obj(vec![("ok", Value::Bool(true)), ("detail", Value::str("saved; rebooting to apply"))]),
    );
    // Reboot to apply (rcS.local connects with the new config on boot — simplest & reliable).
    sh("(sleep 2; reboot) &");
}

fn api_ir_test(s: &mut TcpStream, hal_addr: &str) {
    // Fire hal's built-in 40 kHz IR test waveform (/ir/ir_test — no learned code needed)
    // and report hal's response code. This PHYSICALLY emits IR: point a phone camera at the
    // front IR window to see it flicker.
    let mut be = LtcpBackend::new(hal_addr);
    match be.ir_test(1000, 7) {
        Ok(r) => json_resp(
            s,
            "200 OK",
            &obj(vec![
                ("ok", Value::Bool(r.ok())),
                ("detail", Value::str(format!("ir_test {}", r.detail()))),
            ]),
        ),
        Err(e) => json_resp(
            s,
            "502 Bad Gateway",
            &obj(vec![("ok", Value::Bool(false)), ("detail", Value::str(format!("hal: {}", e)))]),
        ),
    }
}

fn api_ir_loopback(s: &mut TcpStream, hal_addr: &str) {
    // Closed-loop RF self-test: hal emits + receives its own pattern and validates the
    // carrier. No camera needed. Report hal's response verbatim.
    let mut be = LtcpBackend::new(hal_addr);
    match be.ir_loopback() {
        Ok(r) => json_resp(
            s,
            "200 OK",
            &obj(vec![
                ("ok", Value::Bool(r.ok())),
                ("detail", Value::str(format!("loopback {}", r.detail()))),
            ]),
        ),
        Err(e) => json_resp(
            s,
            "502 Bad Gateway",
            &obj(vec![("ok", Value::Bool(false)), ("detail", Value::str(format!("hal: {}", e)))]),
        ),
    }
}

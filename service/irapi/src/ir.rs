//! IR abstraction: the `IrBackend` trait (the swap seam) + `IrCode` model, the
//! production `LtcpBackend` (talks to the stock `hal` on 127.0.0.1:16716), and a
//! host-side `MockBackend`.
//!
//! Everything here is single-threaded and std-only (the kernel has no futex).
//!
//! The LTCP wire protocol was reverse-engineered from the decompiled stock Lua
//! (ltcp.lua / hbus.lua / hal/main.lua / irsender.lua / irmanager.lua) cross-checked
//! against the C `hal` disassembly — see docs/ir-protocol-reverse-engineering.raw.txt.
//! Key facts: connect to 127.0.0.1:16716, send ONE 0x01 hello byte, then LTCP frames;
//! NO checksum; a "command" (opcode 8) carries `{"id","cmd","data"}` JSON directly
//! concatenated with an optional binary IR blob.

use crate::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// An IR code as we store/transport it. v1 uses the opaque "ltcp_blob" encoding:
/// the compiled-IR bytes appended after the JSON envelope on /ir/ir_send.
#[derive(Clone, Debug)]
pub struct IrCode {
    pub encoding: String, // "ltcp_blob"
    pub carrier_hz: u32,  // informational; the real carrier is embedded in the blob header
    pub duty: u8,         // informational; hal hardcodes 50%
    pub ports: Vec<u8>,   // port ids (0=front blaster, 1/2=extender jacks — bit map TBC)
    pub repeat: u16,
    pub blob: Vec<u8>, // the exact bytes appended after the JSON envelope
}

impl IrCode {
    /// Fold port ids into the LTCP `ports` int bitmask. CONFIRMED: hal takes `ports`
    /// as a single integer masked &7 (default 7 = all three emitters). `[0,1,2]->7`.
    /// NOTE: bit->physical-emitter map still unconfirmed (fire masks 1/2/4 & watch).
    pub fn port_mask(&self) -> u32 {
        self.ports.iter().fold(0u32, |m, &p| m | (1u32 << (p as u32)))
    }
}

#[derive(Debug)]
pub enum IrError {
    Io(String),
    Protocol(String),
    Backend(String),
    Timeout,
    Busy,
}

impl std::fmt::Display for IrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IrError::Io(s) => write!(f, "io: {}", s),
            IrError::Protocol(s) => write!(f, "protocol: {}", s),
            IrError::Backend(s) => write!(f, "backend: {}", s),
            IrError::Timeout => write!(f, "timeout"),
            IrError::Busy => write!(f, "busy"),
        }
    }
}

impl From<std::io::Error> for IrError {
    fn from(e: std::io::Error) -> Self {
        map_io(e)
    }
}

fn map_io(e: std::io::Error) -> IrError {
    use std::io::ErrorKind::*;
    match e.kind() {
        WouldBlock | TimedOut => IrError::Timeout,
        _ => IrError::Io(e.to_string()),
    }
}

pub type SessionId = u64;

pub enum LearnState {
    Pending,
    Captured(IrCode),
    Expired,
}

pub struct BackendHealth {
    pub connected: bool,
    pub detail: String,
}

/// The swap seam. LtcpBackend (prod), MockBackend (host), RawSpiBackend (future).
pub trait IrBackend {
    fn send(&mut self, code: &IrCode) -> Result<(), IrError>;
    fn learn_start(&mut self, port: Option<u8>, timeout_s: u16) -> Result<SessionId, IrError>;
    fn learn_poll(&mut self) -> Result<LearnState, IrError>;
    fn learn_stop(&mut self) -> Result<(), IrError>;
    fn health(&mut self) -> BackendHealth;
}

// ---------------------------------------------------------------------------
// Parsed LTCP reply (hal's response to a command)
// ---------------------------------------------------------------------------

/// A decoded LTCP reply. `json` is the response envelope text `{"id":N,"code":C,...}`
/// and `binary` is any trailing binary tail (e.g. an ir_cap capture).
#[derive(Debug, Clone)]
pub struct Reply {
    pub req_id: u8,
    pub is_resp: bool,
    pub is_error: bool,
    pub is_eof: bool,
    pub msg_type: u8,
    pub count: u64,
    pub json: Vec<u8>,
    pub binary: Vec<u8>,
}

impl Reply {
    pub fn json_str(&self) -> String {
        String::from_utf8_lossy(&self.json).into_owned()
    }
    /// hal's response `code` (200 = OK, 500 = "RF timed out", 503 = "RF link lost").
    pub fn code(&self) -> Option<i64> {
        json::parse(&self.json_str()).ok()?.get("code").and_then(|v| v.as_i64())
    }
    /// Some layers add an errorCode; parse defensively (string or int).
    pub fn error_code(&self) -> Option<String> {
        let v = json::parse(&self.json_str()).ok()?;
        let e = v.get("errorCode")?;
        e.as_str().map(|s| s.to_string()).or_else(|| e.as_i64().map(|n| n.to_string()))
    }
    pub fn ok(&self) -> bool {
        self.code() == Some(200) && self.error_code().map_or(true, |c| c == "200")
    }
    /// One-line human summary for CLI/web.
    pub fn detail(&self) -> String {
        let code = self.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        let tail = if self.binary.is_empty() {
            String::new()
        } else {
            format!(", {} binary bytes", self.binary.len())
        };
        format!("code={} body={}{}", code, self.json_str(), tail)
    }
}

/// Read one byte or map a socket timeout to IrError::Timeout.
fn rd1<R: Read>(r: &mut R) -> Result<u8, IrError> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).map_err(map_io)?;
    Ok(b[0])
}

/// Index just past the top-level `{...}` in `b` (brace scan honoring string escapes).
fn json_end(b: &[u8]) -> usize {
    let start = match b.iter().position(|&c| c == b'{') {
        Some(i) => i,
        None => return 0,
    };
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for i in start..b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
        }
    }
    b.len()
}

/// Parse one complete LTCP reply from `s` (primary header + params + secondary
/// packets). Mirrors ltcp.lua processPrimaryPacket/processSecondaryPacket. Uses
/// read_exact everywhere — never trusts a single read() (hal writes tiny segments).
fn read_reply<R: Read>(s: &mut R) -> Result<Reply, IrError> {
    let mut h = [0u8; 4];
    s.read_exact(&mut h).map_err(map_io)?;
    if h[0] != SERVICE_ID {
        return Err(IrError::Protocol(format!("bad serviceId {:#x}", h[0])));
    }
    let (msg_type, req_id, np) = (h[1], h[2], h[3]);
    let is_resp = req_id & 0x80 != 0;
    let is_error = req_id == 0xFF;
    let is_eof = req_id == 0xFE;
    let has_cksum = np & 0x80 != 0;

    let mut nums = Vec::new();
    for _ in 0..(np & 0x3F) {
        let lb = rd1(s)?;
        let w = (lb & 0x3F) as usize;
        if w == 0 {
            // NUL-terminated string param — consume through the 0 byte
            while rd1(s)? != 0 {}
        } else {
            let mut v = 0u64;
            for _ in 0..w {
                v = (v << 8) | rd1(s)? as u64;
            }
            nums.push(v);
        }
    }
    if has_cksum {
        let _ = rd1(s)?;
    }
    if is_error {
        return Err(IrError::Backend("ltcp transport error packet (reqId=0xFF)".into()));
    }
    let count = *nums.get(0).unwrap_or(&1);
    let sec = if is_eof { 0 } else { count.saturating_sub(1) };

    let mut body = Vec::new();
    for _ in 0..sec {
        let _seq = rd1(s)?;
        let lb = rd1(s)?;
        let len = if lb & 0x40 != 0 {
            (((lb & 0x3F) as usize) << 8) | (rd1(s)? as usize)
        } else {
            (lb & 0x3F) as usize
        };
        let mut buf = vec![0u8; len];
        s.read_exact(&mut buf).map_err(map_io)?;
        body.extend_from_slice(&buf);
        if has_cksum {
            let _ = rd1(s)?;
        }
    }

    let jend = json_end(&body);
    let (json, binary) = body.split_at(jend.min(body.len()));
    Ok(Reply {
        req_id,
        is_resp,
        is_error,
        is_eof,
        msg_type,
        count,
        json: json.to_vec(),
        binary: binary.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// LTCP backend
// ---------------------------------------------------------------------------

pub const SERVICE_ID: u8 = 0xFF;
pub const CMD_OPEN: u8 = 1;
pub const CMD_CLOSE: u8 = 7;
pub const CMD_COMMAND: u8 = 8;

pub struct LtcpBackend {
    addr: String,
    stream: Option<TcpStream>,
    msg_id: u8,  // 0..63 per-packet counter (ltcp.lua get_message_id)
    req_id: u16, // 1..65535 hbus request id (the JSON "id")
    timeout: Duration,
}

impl LtcpBackend {
    pub fn new(addr: &str) -> Self {
        LtcpBackend {
            addr: addr.to_string(),
            stream: None,
            msg_id: 0,
            req_id: 0,
            timeout: Duration::from_millis(3000),
        }
    }

    fn connect(&mut self) -> Result<(), IrError> {
        if self.stream.is_none() {
            let mut s = TcpStream::connect(&self.addr)?;
            s.set_nodelay(true).ok();
            s.set_read_timeout(Some(self.timeout)).ok();
            s.set_write_timeout(Some(self.timeout)).ok();
            // hal's connection handshake: tasks/hal/main.lua startTcpConnection sends a
            // single 0x01 byte right after connecting, before any LTCP frames. Skip it and
            // hal reads our 0xFF service-id byte as that hello, mis-frames, and RSTs us.
            s.write_all(&[0x01])?;
            s.flush()?;
            self.stream = Some(s);
        }
        Ok(())
    }

    fn next_msg_id(&mut self) -> u8 {
        let id = self.msg_id;
        self.msg_id = (self.msg_id + 1) & 0x3F;
        id
    }

    fn next_req_id(&mut self) -> u16 {
        self.req_id = self.req_id.wrapping_add(1);
        if self.req_id == 0 {
            self.req_id = 1;
        }
        self.req_id
    }

    /// Write a framed byte sequence and read exactly one complete LTCP reply.
    fn send_frame(&mut self, frame: &[u8]) -> Result<Reply, IrError> {
        self.connect()?;
        let mut s = self.stream.take().unwrap();
        let r = (|| -> Result<Reply, IrError> {
            s.write_all(frame)?;
            s.flush()?;
            read_reply(&mut s)
        })();
        match r {
            Ok(reply) => {
                self.stream = Some(s);
                Ok(reply)
            }
            Err(e) => Err(e), // drop the stream so the next call reconnects (+re-hello)
        }
    }

    /// Send a raw pre-framed byte sequence (for `blast --raw-hex`) and parse the reply.
    pub fn send_raw(&mut self, frame: &[u8]) -> Result<Reply, IrError> {
        self.send_frame(frame)
    }

    /// Issue an LTCP `command` (opcode 8): `{"id":N,"cmd":path,"data":<data_json>}`
    /// directly concatenated with `blob`. This is the single seam for /ir/ir_send,
    /// /ir/ir_test, /ir/ir_loopback and Model-B /ir/ir_cap.
    pub fn command(&mut self, path: &str, data_json: &str, blob: &[u8]) -> Result<Reply, IrError> {
        let rid = self.next_req_id();
        let json = format!("{{\"id\":{},\"cmd\":\"{}\",\"data\":{}}}", rid, path, data_json);
        let mut usr = json.into_bytes();
        usr.extend_from_slice(blob);
        let frame = self.build_command_frame(&usr);
        self.send_frame(&frame)
    }

    /// Build the /ir/ir_send frame for a code (JSON envelope + compiled blob). Exposed
    /// so `blast --dry-run` can print the exact bytes.
    pub fn build_ir_send_frame(&mut self, code: &IrCode) -> Vec<u8> {
        let rid = self.next_req_id();
        let json = format!(
            "{{\"id\":{},\"cmd\":\"/ir/ir_send\",\"data\":{{\"enable\":true,\"keyLatency\":500,\"ports\":{}}}}}",
            rid,
            code.port_mask()
        );
        let mut usr = json.into_bytes();
        usr.extend_from_slice(&code.blob);
        self.build_command_frame(&usr)
    }

    /// Frame an LTCP "command" (opcode 8) carrying `usr` as primary + secondary packets
    /// (ltcp.lua formLtcpPacket). Primary = [FF,08,mid,01,01,noPkt]. Each secondary =
    /// [mid, 0x80|len] for len<=63, or [mid, 0xC0|(len>>8 & 0x3F), len&0xFF] for len>63,
    /// then `len` payload bytes. MAX_PKT=16383 bytes/secondary. No checksum.
    pub fn build_command_frame(&mut self, usr: &[u8]) -> Vec<u8> {
        const MAX_PKT: usize = 16383;
        let sec = if usr.is_empty() { 0 } else { (usr.len() + MAX_PKT - 1) / MAX_PKT };
        let no_pkt = 1 + sec;
        let mid1 = self.next_msg_id();
        let mut out = vec![SERVICE_ID, CMD_COMMAND, mid1, 0x01, 0x01, no_pkt as u8];
        let mut off = 0usize;
        for i in 0..sec {
            let len = if i == sec - 1 { usr.len() - off } else { MAX_PKT };
            let mid = self.next_msg_id();
            if len > 63 {
                out.push(mid);
                out.push(0xC0 | (((len >> 8) & 0x3F) as u8));
                out.push((len & 0xFF) as u8);
            } else {
                out.push(mid);
                out.push(0x80 | (len as u8));
            }
            out.extend_from_slice(&usr[off..off + len]);
            off += len;
        }
        out
    }

    /// Fire a compiled IR code via /ir/ir_send. Returns hal's parsed reply.
    pub fn send_ir(&mut self, code: &IrCode) -> Result<Reply, IrError> {
        let data = format!(
            "{{\"enable\":true,\"keyLatency\":500,\"ports\":{}}}",
            code.port_mask()
        );
        self.command("/ir/ir_send", &data, &code.blob)
    }

    /// Self-test: hal emits its built-in 40 kHz test waveform (no blob needed). Point a
    /// phone camera at the front IR window to see it flicker. `time` is REQUIRED by hal.
    pub fn ir_test(&mut self, time: u32, ports: u32) -> Result<Reply, IrError> {
        let data = format!("{{\"time\":{},\"ports\":{}}}", time, ports);
        self.command("/ir/ir_test", &data, &[])
    }

    /// RF-path self-test: hal emits its 62.5 kHz pattern, captures it back, and validates
    /// the carrier — a closed-loop check that needs NO camera/external receiver.
    pub fn ir_loopback(&mut self) -> Result<Reply, IrError> {
        self.command("/ir/ir_loopback", "{}", &[])
    }

    /// RE helper: open a FRESH connection (optional 0x01 hello), send each frame in
    /// sequence draining the socket for `gap_ms` after each, and return the raw reply
    /// bytes per frame. For interactive LTCP reversing — OPEN/DEVCTL/CLOSE on one socket,
    /// loopback with a long listen window, etc. The connection is dropped afterwards.
    pub fn raw_exchange(
        &mut self,
        frames: &[Vec<u8>],
        gap_ms: u64,
        hello: bool,
    ) -> Result<Vec<Vec<u8>>, IrError> {
        let mut s = TcpStream::connect(&self.addr)?;
        s.set_nodelay(true).ok();
        s.set_read_timeout(Some(Duration::from_millis(300))).ok();
        s.set_write_timeout(Some(self.timeout)).ok();
        if hello {
            s.write_all(&[0x01])?;
            s.flush()?;
        }
        let mut out = Vec::new();
        let mut b = [0u8; 512];
        for f in frames {
            s.write_all(f)?;
            s.flush()?;
            let deadline = Instant::now() + Duration::from_millis(gap_ms);
            let mut buf = Vec::new();
            loop {
                match s.read(&mut b) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&b[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => return Err(map_io(e)),
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
            out.push(buf);
        }
        Ok(out)
    }

    /// DIAGNOSTIC (learn/ir_cap, Model B): send a /ir/ir_cap command, then DRAIN the
    /// socket for `listen_ms` and return every byte received. Used to reverse the capture
    /// stream framing live (hal ACKs then streams 64-byte data frames with buf[0]=0x90,
    /// which are NOT standard [FF,..] response packets — hence a raw dump, not read_reply).
    pub fn ir_cap_probe(
        &mut self,
        time_ms: u32,
        ports: u32,
        listen_ms: u64,
    ) -> Result<Vec<u8>, IrError> {
        self.connect()?;
        let rid = self.next_req_id();
        let json = format!(
            "{{\"id\":{},\"cmd\":\"/ir/ir_cap\",\"data\":{{\"time\":{},\"ports\":{}}}}}",
            rid, time_ms, ports
        );
        let frame = self.build_command_frame(json.as_bytes());
        let mut s = self.stream.take().unwrap();
        let r = (|| -> Result<Vec<u8>, IrError> {
            s.write_all(&frame)?;
            s.flush()?;
            s.set_read_timeout(Some(Duration::from_millis(400))).ok();
            let deadline = Instant::now() + Duration::from_millis(listen_ms);
            let mut all = Vec::new();
            let mut buf = [0u8; 512];
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => all.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        if Instant::now() >= deadline {
                            break;
                        }
                    }
                    Err(e) => return Err(map_io(e)),
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
            Ok(all)
        })();
        // ir_cap mutates hal's connection state; always drop the socket afterwards.
        drop(s);
        self.stream = None;
        r
    }
}

/// Compile a raw mark/space timing sequence into hal's /ir/ir_send blob format
/// (CONFIRMED from irmanager.lua generateFromRawIr + the built-in ir_test waveform).
/// `seq` = list of (is_mark, microseconds). Reproduces hal's own 22-byte test blob for
/// compile_ir_blob(0x01, 40000, 4, &[(true,750),(false,750)]).
pub fn compile_ir_blob(type_byte: u8, carrier_hz: u32, min_repeat: u8, seq: &[(bool, u32)]) -> Vec<u8> {
    // Each interval -> uint16 BE word: bit15 = mark, bits14..0 = µs (min 30). Values
    // >32767 are emitted as 0x7FFF chunks (with the mark bit) until the remainder fits.
    let mut words: Vec<u16> = Vec::new();
    for &(mark, us) in seq {
        let markbit: u16 = if mark { 0x8000 } else { 0 };
        let mut rem = us.max(30);
        while rem > 32767 {
            words.push(markbit | 0x7FFF);
            rem -= 32767;
        }
        if rem < 30 {
            rem = 30;
        }
        words.push(markbit | (rem as u16 & 0x7FFF));
    }
    let period_ns = (1_000_000_000u32 / carrier_hz.max(1)) as u32;
    let mut b = Vec::with_capacity(18 + words.len() * 2);
    b.push(type_byte); // 0x01 keycode / 0xFF raw
    b.extend_from_slice(&period_ns.to_be_bytes());
    b.push(0x32); // duty 50%
    b.extend_from_slice(&0u16.to_be_bytes()); // preSilence
    b.push(min_repeat);
    b.push(0x00); // reserved
    b.extend_from_slice(&0u16.to_be_bytes()); // startLocation
    b.extend_from_slice(&16u16.to_be_bytes()); // repeatLocation -> the sequence at off 16
    b.extend_from_slice(&0u16.to_be_bytes()); // finishLocation
    b.extend_from_slice(&(words.len() as u16).to_be_bytes()); // wordCount
    for w in words {
        b.extend_from_slice(&w.to_be_bytes());
    }
    b
}

impl IrBackend for LtcpBackend {
    fn send(&mut self, code: &IrCode) -> Result<(), IrError> {
        let r = self.send_ir(code)?;
        if r.ok() {
            Ok(())
        } else {
            Err(IrError::Backend(r.detail()))
        }
    }
    fn learn_start(&mut self, _port: Option<u8>, _timeout_s: u16) -> Result<SessionId, IrError> {
        // Live-iteration path is ir_cap_probe (see cmd_learn); a structured learn_start/poll
        // lands once the capture stream framing is confirmed on-device.
        Err(IrError::Backend("use ir_cap_probe (learn diagnostic) — structured learn pending live RE".into()))
    }
    fn learn_poll(&mut self) -> Result<LearnState, IrError> {
        Err(IrError::Backend("learn not yet implemented".into()))
    }
    fn learn_stop(&mut self) -> Result<(), IrError> {
        Ok(())
    }
    fn health(&mut self) -> BackendHealth {
        match self.connect() {
            Ok(()) => BackendHealth {
                connected: true,
                detail: format!("connected to {}", self.addr),
            },
            Err(e) => BackendHealth {
                connected: false,
                detail: e.to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Mock backend (host dev / tests)
// ---------------------------------------------------------------------------

pub struct MockBackend {
    pub sent: Vec<IrCode>,
}

impl MockBackend {
    pub fn new() -> Self {
        MockBackend { sent: Vec::new() }
    }
}

impl IrBackend for MockBackend {
    fn send(&mut self, code: &IrCode) -> Result<(), IrError> {
        eprintln!(
            "[mock] send {} bytes, ports={:?} (mask {:#x}), repeat={}, carrier={}Hz",
            code.blob.len(),
            code.ports,
            code.port_mask(),
            code.repeat,
            code.carrier_hz
        );
        self.sent.push(code.clone());
        Ok(())
    }
    fn learn_start(&mut self, _port: Option<u8>, _timeout_s: u16) -> Result<SessionId, IrError> {
        Ok(1)
    }
    fn learn_poll(&mut self) -> Result<LearnState, IrError> {
        Ok(LearnState::Captured(IrCode {
            encoding: "ltcp_blob".into(),
            carrier_hz: 38000,
            duty: 33,
            ports: vec![0],
            repeat: 1,
            blob: b"MOCKBLOB".to_vec(),
        }))
    }
    fn learn_stop(&mut self) -> Result<(), IrError> {
        Ok(())
    }
    fn health(&mut self) -> BackendHealth {
        BackendHealth {
            connected: true,
            detail: "mock".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_reproduces_builtin_ir_test_blob() {
        // hal's built-in /ir/ir_test waveform (0x463470): 40 kHz, min_repeat 4,
        // one mark + one space of 750 µs each.
        let got = compile_ir_blob(0x01, 40000, 4, &[(true, 750), (false, 750)]);
        let want = [
            0x01, 0x00, 0x00, 0x61, 0xA8, 0x32, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x10,
            0x00, 0x00, 0x00, 0x02, 0x82, 0xEE, 0x02, 0xEE,
        ];
        assert_eq!(got, want, "compiled blob must match hal's built-in test waveform");
    }

    #[test]
    fn json_end_splits_body() {
        let b = br#"{"id":1,"code":200}TRAILING"#;
        let j = json_end(b);
        assert_eq!(&b[..j], br#"{"id":1,"code":200}"#);
        assert_eq!(&b[j..], b"TRAILING");
    }
}

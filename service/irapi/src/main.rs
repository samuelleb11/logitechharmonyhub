//! irapi — standalone IR-over-HTTP appliance for the Harmony Hub.
//!
//! Single-threaded, static mips-musl, zero runtime crates (the kernel has no futex).
//! M0/M1 scaffold: CLI to fire/learn IR through the stock `hal` over LTCP. The HTTP
//! server, device DB, and full REST API land in M2–M4 (see docs/ir-service-buildplan.md).

mod db;
mod enc;
mod i2s;
mod ir;
mod json;
mod mem;
mod util;
mod web;

use ir::{IrBackend, IrCode, LtcpBackend, MockBackend};

const DEFAULT_HAL: &str = "127.0.0.1:16716";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let rest = &args[args.len().min(2)..];
    match cmd {
        "blast" => cmd_blast(rest),
        "selftest" => cmd_selftest(rest),
        "loopback" => cmd_loopback(rest),
        "rawsend" => cmd_rawsend(rest),
        "i2stest" => cmd_i2stest(rest),
        "i2sraw" => cmd_i2sraw(rest),
        "i2sgpiosweep" => cmd_i2sgpiosweep(rest),
        "fire" => cmd_fire(rest),
        "codes" => cmd_codes(rest),
        "peek" => cmd_peek(rest),
        "poke" => cmd_poke(rest),
        "regs" => cmd_regs(rest),
        "learn" => cmd_learn(rest),
        "health" => cmd_health(rest),
        "mock" => cmd_mock(rest),
        "serve" => cmd_serve(rest),
        "devshell" => cmd_devshell(rest),
        "dhcpd" => cmd_dhcpd(rest),
        "" | "-h" | "--help" | "help" => {
            usage();
            if cmd.is_empty() {
                2
            } else {
                0
            }
        }
        other => {
            eprintln!("unknown command '{}'\n", other);
            usage();
            2
        }
    }
}

fn usage() {
    eprintln!(
        "irapi {} — Harmony Hub IR appliance\n\
         \n\
         USAGE:\n\
         \x20 irapi blast    [--host H] (--blob-b64 B | --blob-hex H | --raw-hex H)\n\
         \x20                [--ports 0,1,2] [--repeat 1] [--carrier 38000] [--duty 33]\n\
         \x20 irapi selftest [--host H] [--time 1000] [--ports 7]   # hal's built-in 40kHz test emit\n\
         \x20 irapi loopback [--host H]                             # closed-loop RF self-test (no camera)\n\
         \x20 irapi learn    [--host H] [--time 5000] [--port 0] [--listen 8000]  # ir_cap capture dump\n\
         \x20 irapi health   [--host H]\n\
         \x20 irapi mock                 # exercise the stack with a MockBackend (no device)\n\
         \x20 irapi serve  ...           # (M2) HTTP/JSON API\n\
         \n\
         Default --host = {}",
        env!("CARGO_PKG_VERSION"),
        DEFAULT_HAL
    );
}

// ---- arg helpers (no clap) ----
fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).map(|s| s.as_str());
        }
        let pfx = format!("{}=", name);
        if let Some(v) = args[i].strip_prefix(&pfx) {
            return Some(v);
        }
        i += 1;
    }
    None
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn parse_ports(s: &str) -> Vec<u8> {
    s.split(',')
        .filter_map(|p| p.trim().parse::<u8>().ok())
        .collect()
}

fn cmd_blast(args: &[String]) -> i32 {
    let host = opt(args, "--host").unwrap_or(DEFAULT_HAL);
    let mut be = LtcpBackend::new(host);

    if let Some(raw) = opt(args, "--raw-hex") {
        // M1: replay exact captured wire bytes.
        match util::hex_decode(raw) {
            Ok(bytes) => match be.send_raw(&bytes) {
                Ok(reply) => {
                    println!("sent {} raw bytes; reply {}", bytes.len(), reply.detail());
                    0
                }
                Err(e) => {
                    eprintln!("send error: {}", e);
                    1
                }
            },
            Err(e) => {
                eprintln!("bad --raw-hex: {}", e);
                2
            }
        }
    } else {
        let blob = match (opt(args, "--blob-b64"), opt(args, "--blob-hex")) {
            (Some(b), _) => util::b64_decode(b),
            (_, Some(h)) => util::hex_decode(h),
            (None, None) => {
                eprintln!("blast needs --blob-b64, --blob-hex, or --raw-hex");
                return 2;
            }
        };
        let blob = match blob {
            Ok(b) => b,
            Err(e) => {
                eprintln!("bad blob: {}", e);
                return 2;
            }
        };
        let code = IrCode {
            encoding: "ltcp_blob".into(),
            carrier_hz: opt(args, "--carrier").and_then(|s| s.parse().ok()).unwrap_or(38000),
            duty: opt(args, "--duty").and_then(|s| s.parse().ok()).unwrap_or(33),
            ports: {
                let p = parse_ports(opt(args, "--ports").unwrap_or("0,1,2"));
                if p.is_empty() {
                    vec![0, 1, 2]
                } else {
                    p
                }
            },
            repeat: opt(args, "--repeat").and_then(|s| s.parse().ok()).unwrap_or(1),
            blob,
        };
        if flag(args, "--dry-run") {
            let frame = be.build_ir_send_frame(&code);
            println!(
                "[dry-run] ports={:?} mask={:#x}; provisional frame ({} bytes):\n{}",
                code.ports,
                code.port_mask(),
                frame.len(),
                util::hex_encode(&frame)
            );
            return 0;
        }
        eprintln!(
            "[blast] {} byte blob, ports={:?} (mask {:#x}), repeat={} -> {}",
            code.blob.len(),
            code.ports,
            code.port_mask(),
            code.repeat,
            host
        );
        match be.send_ir(&code) {
            Ok(reply) => {
                println!(
                    "blast {}; reply {}",
                    if reply.ok() { "ok" } else { "NON-200" },
                    reply.detail()
                );
                if reply.ok() {
                    0
                } else {
                    1
                }
            }
            Err(e) => {
                eprintln!("blast error: {}", e);
                1
            }
        }
    }
}

/// Self-test: hal emits its built-in 40 kHz IR pattern (no learned code). Point a phone
/// camera at the front IR window to confirm the emitter physically fires.
fn cmd_selftest(args: &[String]) -> i32 {
    let host = opt(args, "--host").unwrap_or(DEFAULT_HAL);
    let time: u32 = opt(args, "--time").and_then(|s| s.parse().ok()).unwrap_or(1000);
    let ports: u32 = opt(args, "--ports").and_then(|s| s.parse().ok()).unwrap_or(7);
    let mut be = LtcpBackend::new(host);
    eprintln!(
        "[selftest] /ir/ir_test time={} ports={} -> {}  (watch the front IR window w/ a phone camera)",
        time, ports, host
    );
    match be.ir_test(time, ports) {
        Ok(r) => {
            println!("ir_test reply: {}", r.detail());
            if r.ok() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("ir_test error: {}", e);
            1
        }
    }
}

/// Closed-loop RF self-test: hal emits its 62.5 kHz pattern and receives it back to
/// validate the carrier. Needs no camera/external receiver.
fn cmd_loopback(args: &[String]) -> i32 {
    let host = opt(args, "--host").unwrap_or(DEFAULT_HAL);
    let mut be = LtcpBackend::new(host);
    eprintln!("[loopback] /ir/ir_loopback (closed-loop RF self-test) -> {}", host);
    match be.ir_loopback() {
        Ok(r) => {
            println!("ir_loopback reply: {}", r.detail());
            if r.ok() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("ir_loopback error: {}", e);
            1
        }
    }
}

fn parse_u32_maybe_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

/// DIRECT IR TX (bypasses hal): blast a continuous carrier out /dev/i2s so we can see the
/// emitter on a phone camera. hal MUST be stopped first (it holds /dev/i2s exclusively).
fn cmd_i2stest(args: &[String]) -> i32 {
    let carrier: u32 = opt(args, "--carrier").and_then(parse_u32_maybe_hex).unwrap_or(38000);
    let ms: u32 = opt(args, "--ms").and_then(|s| s.parse().ok()).unwrap_or(1000);
    // emitter selector 0..7 (bit0/1/2 -> the three IR outputs); kernel maps bits to GPIO.
    let select: u32 = opt(args, "--select")
        .or_else(|| opt(args, "--ports"))
        .and_then(parse_u32_maybe_hex)
        .unwrap_or(7);
    // The minimal-boot audio reference is ~16x higher than the divider math assumes, so BCLK
    // runs ~16x fast. We compensate by UPSAMPLING the bitstream 16x (--bitmul) while keeping
    // the small, hardware-valid divider (multiplying the divider instead stalls the serializer).
    let bitmul: u32 = opt(args, "--bitmul")
        .or_else(|| opt(args, "--clkmul"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(16);
    // one continuous MARK for `ms`. NOTE at bitmul=16 the buffer is 16x bigger; keep ms small
    // (<~400ms) so it fits the 393KB DMA ring and write() can't block on a stalled clock.
    let seq = [(true, ms * 1000)];
    let (clk, samples) = i2s::render(carrier, 50, bitmul, &seq);
    // SAFE by default: Hold (no blocking DRAIN, can't wedge the kernel). --drain opts into the
    // blocking DRAIN only once the divider is known-good.
    let finish = if flag(args, "--drain") {
        i2s::Finish::Drain
    } else {
        i2s::Finish::Hold(ms as u64 + 300)
    };
    eprintln!(
        "[i2stest] {}Hz carrier, {}ms, select={}, clkdiv={:#x}, bitmul={}, {} sample bytes",
        carrier, ms, select, clk, bitmul, samples.len()
    );
    match i2s::blast_samples(clk, select, &samples, finish) {
        Ok(()) => {
            println!("i2stest: blasted ok (drained)");
            0
        }
        Err(e) => {
            eprintln!("i2stest error: {}", e);
            1
        }
    }
}

/// Fire a code from the bundled offline DB by device + function (via the direct I2S driver).
fn cmd_fire(args: &[String]) -> i32 {
    let device = opt(args, "--device").unwrap_or("");
    let function = opt(args, "--function").unwrap_or("");
    let select: u32 = opt(args, "--select").and_then(parse_u32_maybe_hex).unwrap_or(7);
    if device.is_empty() || function.is_empty() {
        eprintln!("fire needs --device <id> --function <name>  (see 'irapi codes')");
        return 2;
    }
    match db::lookup(device, function) {
        Some(r) => {
            let seq = r.seq();
            eprintln!(
                "[fire] {}/{}: raw {} intervals @ {}Hz duty {}",
                device, function, seq.len(), r.carrier, r.duty
            );
            match i2s::blast(r.carrier, r.duty.max(1), 1, select, &seq, i2s::Finish::Drain) {
                Ok(()) => {
                    println!("fired {}/{}", device, function);
                    0
                }
                Err(e) => {
                    eprintln!("fire error: {}", e);
                    1
                }
            }
        }
        None => {
            eprintln!("no code '{}' for device '{}'", function, device);
            2
        }
    }
}

/// List bundled devices, or the functions of one device (--device <id>).
fn cmd_codes(args: &[String]) -> i32 {
    match opt(args, "--device") {
        Some(dev) => {
            for f in db::functions(dev) {
                println!("{}", f);
            }
        }
        None => {
            for d in db::devices(None, None) {
                println!("{}\t{}\t{} {}", d.id, d.dtype, d.brand, d.model);
            }
        }
    }
    0
}

/// Blast an arbitrary mark/space timing sequence (one-shot, so it can't wedge) — drives ANY
/// IR protocol from a host-computed µs list. `--us` alternates mark,space,mark,space,... (µs).
fn cmd_i2sraw(args: &[String]) -> i32 {
    let carrier: u32 = opt(args, "--carrier").and_then(parse_u32_maybe_hex).unwrap_or(38000);
    let select: u32 = opt(args, "--select").and_then(parse_u32_maybe_hex).unwrap_or(7);
    let bitmul: u32 = opt(args, "--bitmul").and_then(|s| s.parse().ok()).unwrap_or(1);
    let us = match opt(args, "--us") {
        Some(u) => u,
        None => {
            eprintln!("i2sraw needs --us mark,space,mark,space,... (microseconds)");
            return 2;
        }
    };
    let mut seq: Vec<(bool, u32)> = Vec::new();
    let mut mark = true;
    let mut total_us: u64 = 0;
    for tok in us.split(',') {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        match t.parse::<u32>() {
            Ok(v) => {
                seq.push((mark, v));
                total_us += v as u64;
                mark = !mark;
            }
            Err(_) => {
                eprintln!("bad --us value: {}", t);
                return 2;
            }
        }
    }
    if seq.is_empty() {
        eprintln!("empty --us sequence");
        return 2;
    }
    // DRAIN by default: return the instant the frame finishes ONE pass, then close — so the
    // looping DMA can't replay/garble it. --hold falls back to the (non-blocking) hold mode.
    let finish = if flag(args, "--hold") {
        i2s::Finish::Hold(total_us / 1000 + 60)
    } else {
        i2s::Finish::Drain
    };
    eprintln!(
        "[i2sraw] {} intervals, {}us total (~{}ms), carrier={} bitmul={} select={}",
        seq.len(), total_us, total_us / 1000, carrier, bitmul, select
    );
    match i2s::blast(carrier, 50, bitmul, select, &seq, finish) {
        Ok(()) => {
            println!("i2sraw: sent");
            0
        }
        Err(e) => {
            eprintln!("i2sraw error: {}", e);
            1
        }
    }
}

/// Sweep the candidate emitter-enable GPIO bits, blasting a carrier burst on each so we can
/// identify which bit drives the physical blaster. hal MUST be stopped first.
fn cmd_i2sgpiosweep(args: &[String]) -> i32 {
    let carrier: u32 = opt(args, "--carrier").and_then(parse_u32_maybe_hex).unwrap_or(38000);
    let ms: u32 = opt(args, "--ms").and_then(|s| s.parse().ok()).unwrap_or(300);
    let bitmul: u32 = opt(args, "--bitmul").and_then(|s| s.parse().ok()).unwrap_or(16);
    let seq = [(true, ms * 1000)];
    let (clk, samples) = i2s::render(carrier, 50, bitmul, &seq);
    // selector values 0..7 -> kernel sets GPIO16/28/13 from bits 0/1/2.
    let candidates: [(u32, &str); 5] = [
        (0, "none (DMA only)"),
        (1, "port0 -> GPIO16"),
        (2, "port1 -> GPIO28"),
        (4, "port2 -> GPIO13"),
        (7, "all three"),
    ];
    for (sel, name) in candidates {
        eprintln!("[sweep] {}ms burst, select={}  <-- {}", ms, sel, name);
        if let Err(e) = i2s::blast_samples(clk, sel, &samples, i2s::Finish::Hold(ms as u64 + 300)) {
            eprintln!("  error: {}", e);
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    println!("sweep done");
    0
}

/// Read one or more 32-bit MMIO registers via /dev/mem.
fn cmd_peek(args: &[String]) -> i32 {
    let addr = match opt(args, "--addr").and_then(parse_u32_maybe_hex) {
        Some(a) => a,
        None => {
            eprintln!("peek needs --addr 0x...");
            return 2;
        }
    };
    let count: u32 = opt(args, "--count").and_then(|s| s.parse().ok()).unwrap_or(1);
    for i in 0..count {
        let a = addr + i * 4;
        match mem::peek(a) {
            Ok(v) => println!("{:#010x} = {:#010x}", a, v),
            Err(e) => {
                eprintln!("peek {:#x}: {}", a, e);
                return 1;
            }
        }
    }
    0
}

/// Write a 32-bit MMIO register via /dev/mem.
fn cmd_poke(args: &[String]) -> i32 {
    let addr = opt(args, "--addr").and_then(parse_u32_maybe_hex);
    let val = opt(args, "--val").and_then(parse_u32_maybe_hex);
    match (addr, val) {
        (Some(a), Some(v)) => match mem::poke(a, v) {
            Ok(()) => {
                println!("poked {:#010x} = {:#010x}", a, v);
                0
            }
            Err(e) => {
                eprintln!("poke: {}", e);
                1
            }
        },
        _ => {
            eprintln!("poke needs --addr 0x... --val 0x...");
            2
        }
    }
}

/// Dump the AR9331 registers relevant to IR TX with their expected values.
fn cmd_regs(_args: &[String]) -> i32 {
    for (a, name, note) in mem::IR_REGS {
        match mem::peek(*a) {
            Ok(v) => println!("{} {:#010x} = {:#010x}   {}", name, a, v, note),
            Err(e) => {
                eprintln!("{} {:#010x} = ERR {}", name, a, e);
                return 1;
            }
        }
    }
    0
}

/// RE Swiss-army tool: send one or more raw LTCP frames (--frame HEX, repeatable) over a
/// single connection (0x01 hello unless --no-hello), draining --gap ms after each, and dump
/// every reply byte. Used to reverse OPEN/DEVCTL/CLOSE choreography live.
fn cmd_rawsend(args: &[String]) -> i32 {
    let host = opt(args, "--host").unwrap_or(DEFAULT_HAL);
    let gap: u64 = opt(args, "--gap").and_then(|s| s.parse().ok()).unwrap_or(600);
    let hello = !flag(args, "--no-hello");
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--frame" {
            match args.get(i + 1).map(|h| util::hex_decode(h)) {
                Some(Ok(b)) => frames.push(b),
                Some(Err(e)) => {
                    eprintln!("bad --frame hex: {}", e);
                    return 2;
                }
                None => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    if frames.is_empty() {
        eprintln!("rawsend needs at least one --frame HEX");
        return 2;
    }
    let mut be = LtcpBackend::new(host);
    match be.raw_exchange(&frames, gap, hello) {
        Ok(replies) => {
            for (i, r) in replies.iter().enumerate() {
                let ascii: String = r
                    .iter()
                    .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
                    .collect();
                println!("frame{} reply {}b: {}", i, r.len(), util::hex_encode(r));
                println!("  ascii: {}", ascii);
            }
            0
        }
        Err(e) => {
            eprintln!("rawsend error: {}", e);
            1
        }
    }
}

/// LEARN diagnostic (/ir/ir_cap, Model B): arm a capture and DUMP every byte hal streams
/// back, so we can reverse the capture-stream framing live. Point a remote at the hub and
/// press a button within the listen window.
fn cmd_learn(args: &[String]) -> i32 {
    let host = opt(args, "--host").unwrap_or(DEFAULT_HAL);
    let time: u32 = opt(args, "--time").and_then(|s| s.parse().ok()).unwrap_or(5000);
    let port: u32 = opt(args, "--port").and_then(|s| s.parse().ok()).unwrap_or(0);
    let listen: u64 = opt(args, "--listen").and_then(|s| s.parse().ok()).unwrap_or(8000);
    let ports = 1u32 << port;
    let mut be = LtcpBackend::new(host);
    eprintln!(
        "[learn] /ir/ir_cap time={} ports={} — POINT A REMOTE AT THE HUB, press a button within {}ms...",
        time, ports, listen
    );
    match be.ir_cap_probe(time, ports, listen) {
        Ok(bytes) => {
            println!("captured {} raw bytes:", bytes.len());
            println!("{}", util::hex_encode(&bytes));
            // best-effort ASCII view of any JSON ack that leads the stream
            let ascii: String = bytes.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
            println!("ascii: {}", ascii);
            0
        }
        Err(e) => {
            eprintln!("learn error: {}", e);
            1
        }
    }
}

fn cmd_health(args: &[String]) -> i32 {
    let host = opt(args, "--host").unwrap_or(DEFAULT_HAL);
    let mut be = LtcpBackend::new(host);
    let h = be.health();
    println!(
        "{{\"backend\":\"ltcp\",\"host\":\"{}\",\"connected\":{},\"detail\":\"{}\"}}",
        host, h.connected, h.detail
    );
    if h.connected {
        0
    } else {
        1
    }
}

/// DEV-ONLY untethered shell: token-authed single-threaded TCP command server.
/// The device has no dropbear; this gives networked shell access on the trusted LAN.
/// Single connection at a time (no threads -> no futex). NOT for production.
fn cmd_devshell(args: &[String]) -> i32 {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let port: u16 = opt(args, "--port").and_then(|s| s.parse().ok()).unwrap_or(2222);
    let token = opt(args, "--token").unwrap_or("harmony").to_string();
    let bind = format!("0.0.0.0:{}", port);
    let listener = match TcpListener::bind(&bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("devshell: bind {} failed: {}", bind, e);
            return 1;
        }
    };
    eprintln!("[devshell] listening on {} (send the token as the first line)", bind);

    for conn in listener.incoming() {
        let mut s = match conn {
            Ok(s) => s,
            Err(_) => continue,
        };
        let reader_stream = match s.try_clone() {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut reader = BufReader::new(reader_stream);

        // auth: first line must equal the token
        let mut first = String::new();
        if reader.read_line(&mut first).is_err() {
            continue;
        }
        if first.trim() != token {
            let _ = s.write_all(b"AUTH FAIL\n");
            continue;
        }
        let _ = s.write_all(b"harmony devshell ready (one command per line; 'exit' to close)\n# ");

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let cmd = line.trim();
            if cmd.is_empty() {
                let _ = s.write_all(b"# ");
                continue;
            }
            if cmd == "exit" {
                break;
            }
            // merge stderr into stdout in the shell so we read ONE pipe (no threads)
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("{} 2>&1", cmd))
                .output();
            match out {
                Ok(o) => {
                    let _ = s.write_all(&o.stdout);
                }
                Err(e) => {
                    let _ = s.write_all(format!("exec error: {}\n", e).as_bytes());
                }
            }
            let _ = s.write_all(b"# ");
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Minimal DHCP server for the AP-fallback mode (device has no udhcpd).
// Single-threaded UDP loop (no futex). Serves a tiny sequential pool on ONE
// interface (SO_BINDTODEVICE) so it can't answer DHCP on the real LAN.
// ---------------------------------------------------------------------------
fn parse_ipv4(s: &str) -> [u8; 4] {
    let mut o = [0u8; 4];
    for (i, part) in s.split('.').take(4).enumerate() {
        o[i] = part.trim().parse().unwrap_or(0);
    }
    o
}

fn bind_to_device(fd: i32, iface: &str) {
    extern "C" {
        fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32;
    }
    // MIPS: SOL_SOCKET = 0xffff, SO_BINDTODEVICE = 25. Non-fatal (ignored off-target).
    const SOL_SOCKET_MIPS: i32 = 0xffff;
    const SO_BINDTODEVICE: i32 = 25;
    let name = iface.as_bytes();
    unsafe {
        setsockopt(fd, SOL_SOCKET_MIPS, SO_BINDTODEVICE, name.as_ptr(), name.len() as u32);
    }
}

fn cmd_dhcpd(args: &[String]) -> i32 {
    use std::net::UdpSocket;
    use std::os::unix::io::AsRawFd;

    let iface = opt(args, "--iface").unwrap_or("ath1").to_string();
    let server = parse_ipv4(opt(args, "--server").unwrap_or("192.168.4.1"));
    let start = parse_ipv4(opt(args, "--start").unwrap_or("192.168.4.10"));
    let count: u32 = opt(args, "--count").and_then(|s| s.parse().ok()).unwrap_or(20);
    let mask = parse_ipv4(opt(args, "--mask").unwrap_or("255.255.255.0"));
    let lease: u32 = opt(args, "--lease").and_then(|s| s.parse().ok()).unwrap_or(3600);
    let port: u16 = opt(args, "--port").and_then(|s| s.parse().ok()).unwrap_or(67);
    let reply_port: u16 = opt(args, "--reply-port").and_then(|s| s.parse().ok()).unwrap_or(68);

    let sock = match UdpSocket::bind(format!("0.0.0.0:{}", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("dhcpd: bind 0.0.0.0:{} failed: {}", port, e);
            return 1;
        }
    };
    sock.set_broadcast(true).ok();
    bind_to_device(sock.as_raw_fd(), &iface);
    eprintln!(
        "[dhcpd] on {} server {}.{}.{}.{} pool {}.{}.{}.{}+{}",
        iface, server[0], server[1], server[2], server[3], start[0], start[1], start[2], start[3], count
    );

    let mut leases: Vec<([u8; 6], [u8; 4])> = Vec::new();
    let mut buf = [0u8; 1500];
    loop {
        let n = match sock.recv_from(&mut buf) {
            Ok((n, _)) => n,
            Err(_) => continue,
        };
        if n < 240 || buf[236..240] != [0x63, 0x82, 0x53, 0x63] {
            continue; // too short / not BOOTP magic cookie
        }
        // parse option 53 (DHCP message type)
        let mut mtype = 0u8;
        let mut i = 240;
        while i + 1 < n {
            let o = buf[i];
            if o == 0xff {
                break;
            }
            if o == 0 {
                i += 1;
                continue;
            }
            let l = buf[i + 1] as usize;
            if i + 2 + l > n {
                break;
            }
            if o == 53 && l >= 1 {
                mtype = buf[i + 2];
            }
            i += 2 + l;
        }
        let reply_type = match mtype {
            1 => 2u8, // DISCOVER -> OFFER
            3 => 5u8, // REQUEST  -> ACK
            _ => continue,
        };
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&buf[28..34]);
        // assign (reuse if known, else next sequential)
        let ip = {
            let mut found = None;
            for (m, ip) in leases.iter() {
                if *m == mac {
                    found = Some(*ip);
                    break;
                }
            }
            found.unwrap_or_else(|| {
                let idx = (leases.len() as u32) % count.max(1);
                let ip = (u32::from_be_bytes(start) + idx).to_be_bytes();
                leases.push((mac, ip));
                ip
            })
        };
        // build BOOTREPLY
        let mut r = vec![0u8; 240];
        r[0] = 2; // BOOTREPLY
        r[1] = buf[1]; // htype
        r[2] = buf[2]; // hlen
        r[4..8].copy_from_slice(&buf[4..8]); // xid
        r[16..20].copy_from_slice(&ip); // yiaddr
        r[20..24].copy_from_slice(&server); // siaddr
        r[28..44].copy_from_slice(&buf[28..44]); // chaddr
        r[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]); // cookie
        r.extend_from_slice(&[53, 1, reply_type]);
        r.extend_from_slice(&[54, 4]);
        r.extend_from_slice(&server); // server id
        r.extend_from_slice(&[51, 4]);
        r.extend_from_slice(&lease.to_be_bytes());
        r.extend_from_slice(&[1, 4]);
        r.extend_from_slice(&mask);
        r.extend_from_slice(&[3, 4]);
        r.extend_from_slice(&server); // router
        r.extend_from_slice(&[6, 4]);
        r.extend_from_slice(&server); // dns
        r.push(0xff);
        let _ = sock.send_to(&r, format!("255.255.255.255:{}", reply_port));
        eprintln!(
            "[dhcpd] {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} -> {}.{}.{}.{} {}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], ip[0], ip[1], ip[2], ip[3],
            if reply_type == 2 { "OFFER" } else { "ACK" }
        );
    }
}

fn cmd_serve(args: &[String]) -> i32 {
    let port: u16 = opt(args, "--port").and_then(|s| s.parse().ok()).unwrap_or(80);
    let hal = opt(args, "--hal").unwrap_or(DEFAULT_HAL).to_string();
    web::serve(port, &hal)
}

fn cmd_mock(_args: &[String]) -> i32 {
    let mut be = MockBackend::new();
    let code = IrCode {
        encoding: "ltcp_blob".into(),
        carrier_hz: 38000,
        duty: 33,
        ports: vec![0, 1, 2],
        repeat: 1,
        blob: b"HELLO-IR".to_vec(),
    };
    let _ = be.send(&code);
    let h = be.health();
    println!("mock backend ok: {}", h.detail);
    0
}

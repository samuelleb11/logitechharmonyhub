// rustprobe: prove a SINGLE-THREADED Rust binary runs on the Hub's 2.6.31 kernel,
// which has CONFIG_FUTEX=n (the reason Go can't run). Rust std only touches futex
// for thread::spawn/join and CONTENDED locks; this program spawns no threads and
// never contends a lock, so it must never call futex. It exercises startup, file
// I/O, and the socket syscalls (socket/bind/connect/send/recv) via a UDP
// round-trip to itself over loopback — all in one thread.
use std::fs;
use std::io::Write;
use std::net::{TcpListener, UdpSocket};

fn main() {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "RUST_START");

    match fs::read_to_string("/proc/version") {
        Ok(v) => {
            let _ = write!(out, "KVER: {}", v);
        }
        Err(e) => {
            let _ = writeln!(out, "KVER_ERR: {}", e);
        }
    }

    // TCP socket+bind+listen (what the real service needs to accept HTTP)
    match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => {
            let _ = writeln!(out, "TCP_LISTEN_OK {}", l.local_addr().unwrap());
        }
        Err(e) => {
            let _ = writeln!(out, "TCP_LISTEN_ERR {}", e);
        }
    }

    // single-threaded UDP round-trip to self over loopback
    match UdpSocket::bind("127.0.0.1:0") {
        Ok(sock) => {
            let addr = sock.local_addr().unwrap();
            let _ = writeln!(out, "UDP_BOUND {}", addr);
            if sock.connect(addr).is_ok() && sock.send(b"ping").is_ok() {
                let mut buf = [0u8; 16];
                match sock.recv(&mut buf) {
                    Ok(n) => {
                        let _ = writeln!(out, "UDP_RECV {}", String::from_utf8_lossy(&buf[..n]));
                    }
                    Err(e) => {
                        let _ = writeln!(out, "UDP_RECV_ERR {}", e);
                    }
                }
            } else {
                let _ = writeln!(out, "UDP_SEND_ERR");
            }
        }
        Err(e) => {
            let _ = writeln!(out, "UDP_BIND_ERR {}", e);
        }
    }

    let _ = writeln!(out, "RUST_RUNS_2631_OK");
}

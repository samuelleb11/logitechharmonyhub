//! On-device IR protocol encoders: (protocol, address, command) -> raw mark/space µs.
//! Ports of the standard/Flipper protocol definitions so we can store compact params in the
//! DB and expand at fire-time (vs. bulky raw µs). Mirrors the timings in the design notes.

pub struct Encoded {
    pub carrier: u32,
    pub duty: u8,
    pub us: Vec<u32>,
}

impl Encoded {
    pub fn seq(&self) -> Vec<(bool, u32)> {
        self.us.iter().enumerate().map(|(i, &v)| (i % 2 == 0, v)).collect()
    }
}

fn lsb(byte: u32, n: u32) -> Vec<bool> {
    (0..n).map(|i| (byte >> i) & 1 == 1).collect()
}

fn le(b: &[u8]) -> u32 {
    let mut v = 0u32;
    for (i, &x) in b.iter().take(4).enumerate() {
        v |= (x as u32) << (8 * i as u32);
    }
    v
}

/// Pulse-distance modulation: header, then per bit a mark + (one|zero) space, then trailer+gap.
fn pdm(hdr: (u32, u32), bit_mark: u32, zero: u32, one: u32, trail: u32, gap: u32, bits: &[bool]) -> Vec<u32> {
    let mut out = vec![hdr.0, hdr.1];
    for &b in bits {
        out.push(bit_mark);
        out.push(if b { one } else { zero });
    }
    out.push(trail);
    out.push(gap);
    out
}

/// Encode a protocol frame. Returns None for unsupported protocols. `addr`/`cmd` are the
/// Flipper little-endian byte fields (up to 4 bytes each).
pub fn encode(protocol: &str, addr: &[u8], cmd: &[u8]) -> Option<Encoded> {
    let a = |i: usize| *addr.get(i).unwrap_or(&0) as u32;
    let c = |i: usize| *cmd.get(i).unwrap_or(&0) as u32;

    match protocol {
        "NEC" => {
            let (ad, cm) = (a(0), c(0));
            let mut bits = lsb(ad, 8);
            bits.extend(lsb(ad ^ 0xFF, 8));
            bits.extend(lsb(cm, 8));
            bits.extend(lsb(cm ^ 0xFF, 8));
            Some(Encoded { carrier: 38000, duty: 33, us: pdm((9000, 4500), 560, 560, 1690, 560, 40000, &bits) })
        }
        "NECext" | "NEC42ext" => {
            let (al, ah, cm) = (a(0), a(1), c(0));
            let mut bits = lsb(al, 8);
            bits.extend(lsb(ah, 8));
            bits.extend(lsb(cm, 8));
            bits.extend(lsb(cm ^ 0xFF, 8));
            Some(Encoded { carrier: 38000, duty: 33, us: pdm((9000, 4500), 560, 560, 1690, 560, 40000, &bits) })
        }
        "NEC42" => {
            let ad = le(addr) & 0x1FFF;
            let cm = c(0);
            let mut bits = lsb(ad, 13);
            bits.extend(lsb((!ad) & 0x1FFF, 13));
            bits.extend(lsb(cm, 8));
            bits.extend(lsb(cm ^ 0xFF, 8));
            Some(Encoded { carrier: 38000, duty: 33, us: pdm((9000, 4500), 560, 560, 1690, 560, 40000, &bits) })
        }
        "Samsung32" => {
            let (ad, cm) = (a(0), c(0));
            let mut bits = lsb(ad, 8);
            bits.extend(lsb(ad, 8));
            bits.extend(lsb(cm, 8));
            bits.extend(lsb(cm ^ 0xFF, 8));
            Some(Encoded { carrier: 38000, duty: 33, us: pdm((4500, 4500), 560, 560, 1690, 560, 40000, &bits) })
        }
        "SIRC" | "SIRC15" | "SIRC20" => {
            let nbits = if protocol == "SIRC" { 12 } else if protocol == "SIRC15" { 15 } else { 20 };
            let cm = c(0) & 0x7F;
            let (ad, abits) = match nbits {
                12 => (a(0) & 0x1F, 5),
                15 => (a(0) & 0xFF, 8),
                _ => (le(addr) & 0x1FFF, 13),
            };
            let mut bits = lsb(cm, 7);
            bits.extend(lsb(ad, abits));
            // pulse-width: header 2400/600, then per bit (one=1200|zero=600) mark + 600 space.
            let mut frame = vec![2400u32, 600];
            for &b in &bits {
                frame.push(if b { 1200 } else { 600 });
                frame.push(600);
            }
            let n = frame.len();
            frame[n - 1] = 26000; // inter-frame gap (~45ms period)
            let mut us = Vec::new();
            for _ in 0..3 {
                us.extend_from_slice(&frame); // Sony repeats 3x
            }
            Some(Encoded { carrier: 40000, duty: 33, us })
        }
        "RC5" | "RC5X" => {
            let ad = a(0) & 0x1F;
            let cm = c(0) & 0x3F;
            let mut bits = vec![true, true, false]; // S1, S2, toggle=0
            for i in (0..5).rev() {
                bits.push((ad >> i) & 1 == 1);
            }
            for i in (0..6).rev() {
                bits.push((cm >> i) & 1 == 1);
            }
            let t = 889u32;
            // Manchester: 1 = space,mark ; 0 = mark,space
            let mut half: Vec<(bool, u32)> = Vec::new();
            for &b in &bits {
                if b {
                    half.push((false, t));
                    half.push((true, t));
                } else {
                    half.push((true, t));
                    half.push((false, t));
                }
            }
            if matches!(half.first(), Some((false, _))) {
                half.remove(0); // start on a mark (line idles low)
            }
            let mut us: Vec<u32> = Vec::new();
            let mut last: Option<bool> = None;
            for (is_mark, d) in half {
                if last == Some(is_mark) {
                    *us.last_mut().unwrap() += d;
                } else {
                    us.push(d);
                    last = Some(is_mark);
                }
            }
            match last {
                Some(true) => us.push(114000),                     // gap as the trailing space
                Some(false) => *us.last_mut().unwrap() += 114000,  // extend trailing space
                None => {}
            }
            Some(Encoded { carrier: 36000, duty: 33, us })
        }
        _ => None,
    }
}

/// Parse a Flipper hex byte field ("EE 87 00 00" or "ee870000") into up to 4 bytes.
pub fn hex_bytes(s: &str) -> Vec<u8> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    clean
        .as_bytes()
        .chunks(2)
        .filter_map(|ch| std::str::from_utf8(ch).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nec_frame_shape() {
        let e = encode("NEC", &[0x04], &[0x08]).unwrap();
        // header(2) + 32 bits*2 + trailer + gap = 2 + 64 + 2 = 68 intervals
        assert_eq!(e.us.len(), 68);
        assert_eq!((e.us[0], e.us[1]), (9000, 4500));
        assert_eq!(e.carrier, 38000);
        assert_eq!(e.seq()[0].0, true);
    }
    #[test]
    fn samsung_header() {
        let e = encode("Samsung32", &[0x07], &[0x02]).unwrap();
        assert_eq!((e.us[0], e.us[1]), (4500, 4500));
        assert_eq!(e.us.len(), 68);
    }
    #[test]
    fn hex_parse() {
        assert_eq!(hex_bytes("EE 87 00 00"), vec![0xEE, 0x87, 0x00, 0x00]);
    }
}

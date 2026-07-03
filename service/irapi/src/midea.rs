//! Native Midea/Danby AC encoder: a full climate state (power/mode/fan/temp°C) -> the 2-frame
//! IR waveform, generated on the fly. Reversed from the IRremoteESP8266 Midea field layout and
//! VALIDATED against real Danby DAC060EB7WDB captures:
//!   on, Cool, High, 17°C -> A1 98 60 FF FF 7A  (= the "Cool_hi" capture)
//!   on, Dry,  Auto, 24°C -> A1 81 67 FF FF 68  (= the "Dh" capture)
//! Frame = 6 bytes [B5=0xA1 header .. B0=checksum], sent MSB-first, then the whole frame is
//! re-sent bit-inverted. 38kHz, pulse-distance.

pub const MODE_COOL: u8 = 0;
pub const MODE_DRY: u8 = 1;
pub const MODE_AUTO: u8 = 2;
pub const MODE_HEAT: u8 = 3;
pub const MODE_FAN: u8 = 4;

pub const FAN_AUTO: u8 = 0;
pub const FAN_LOW: u8 = 1;
pub const FAN_MED: u8 = 2;
pub const FAN_HIGH: u8 = 3;

pub const CARRIER: u32 = 38000;

fn revbits(b: u8) -> u8 {
    let mut r = 0u8;
    for i in 0..8 {
        if b & (1 << i) != 0 {
            r |= 1 << (7 - i);
        }
    }
    r
}

pub fn mode_from_str(s: &str) -> u8 {
    match s.to_ascii_lowercase().as_str() {
        "cool" => MODE_COOL,
        "dry" => MODE_DRY,
        "heat" => MODE_HEAT,
        "fan" | "fan_only" => MODE_FAN,
        _ => MODE_AUTO,
    }
}

pub fn fan_from_str(s: &str) -> u8 {
    match s.to_ascii_lowercase().as_str() {
        "low" => FAN_LOW,
        "medium" | "med" => FAN_MED,
        "high" => FAN_HIGH,
        _ => FAN_AUTO,
    }
}

/// The 6 Midea frame bytes [B5..B0]. B5=0xA1 header; B0=checksum (over B1..B5).
pub fn bytes(power: bool, mode: u8, fan: u8, temp_c: u8) -> [u8; 6] {
    let b5 = 0xA1u8;
    let b4 = ((power as u8) << 7) | ((fan & 0x03) << 3) | (mode & 0x07);
    let temp = temp_c.clamp(17, 31) - 17;
    let b3 = 0x60 | (temp & 0x1F);
    let b2 = 0xFFu8;
    let b1 = 0xFFu8;
    let mut sum: u16 = 0;
    for &b in &[b1, b2, b3, b4, b5] {
        sum += revbits(b) as u16;
    }
    let comp = ((256u16.wrapping_sub(sum % 256)) % 256) as u8;
    let b0 = revbits(comp);
    [b5, b4, b3, b2, b1, b0]
}

/// Encode a Midea state into the mark/space µs waveform (data frame + inverted frame).
pub fn encode(power: bool, mode: u8, fan: u8, temp_c: u8) -> Vec<(bool, u32)> {
    let data = bytes(power, mode, fan, temp_c);
    let inv: [u8; 6] = {
        let mut x = [0u8; 6];
        for i in 0..6 {
            x[i] = !data[i];
        }
        x
    };
    let mut seq: Vec<(bool, u32)> = Vec::new();
    for frame in [&data, &inv] {
        seq.push((true, 4480));
        seq.push((false, 4480));
        for &byte in frame.iter() {
            for k in (0..8).rev() {
                seq.push((true, 560));
                seq.push((false, if byte & (1 << k) != 0 { 1600 } else { 560 }));
            }
        }
        seq.push((true, 560));
        seq.push((false, 5100));
    }
    seq
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reproduces_real_danby_captures() {
        assert_eq!(bytes(true, MODE_COOL, FAN_HIGH, 17), [0xA1, 0x98, 0x60, 0xFF, 0xFF, 0x7A]);
        assert_eq!(bytes(true, MODE_DRY, FAN_AUTO, 24), [0xA1, 0x81, 0x67, 0xFF, 0xFF, 0x68]);
    }
    #[test]
    fn waveform_shape() {
        let w = encode(true, MODE_COOL, FAN_AUTO, 22);
        // 2 frames * (header 2 + 48 bits*2 + footer 2) = 2*100 = 200 intervals
        assert_eq!(w.len(), 200);
        assert_eq!(w[0], (true, 4480));
    }
}

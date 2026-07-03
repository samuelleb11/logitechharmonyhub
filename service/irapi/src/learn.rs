//! IR learn: capture from /dev/i2s (RX) and decode into a mark/space code we can store+replay.

use crate::i2s;

/// Microseconds per captured sample. CALIBRATE on-device (see docs/ir-receive-learn-plan.raw.txt):
/// this board's audio reference runs ~16x fast (same quirk as TX `bit_mul`), so the textbook
/// 1.5µs/sample from the hal disassembly is ≈1.5/16 ≈ 0.094 here. Tune with a known remote
/// (NEC leader mark = 9000µs -> us_per_sample = 9000 / leader_sample_count).
pub const US_PER_SAMPLE: f32 = 0.094;

pub struct Learned {
    pub carrier: u32,
    pub us: Vec<u32>, // alternating mark,space,… starting with a MARK
}

/// Decode a raw capture into the first IR frame. Assumes a demodulated (envelope) receiver —
/// clean marks; default carrier 38kHz (correct for the vast majority of consumer remotes).
pub fn decode(raw: &[u8], us_per_sample: f32) -> Option<Learned> {
    let runs = i2s::rle_decode(raw);
    if runs.len() < 8 {
        return None;
    }
    let mut us: Vec<u32> = runs
        .iter()
        .map(|&(_, c)| ((c as f32 * us_per_sample).round().max(30.0)) as u32)
        .collect();
    // keep the first frame: cut at the first inter-frame gap (> 20 ms)
    if let Some(i) = us.iter().position(|&d| d > 20_000) {
        us.truncate(i);
    }
    // must start on a mark and have a plausible frame
    if us.len() < 8 {
        return None;
    }
    Some(Learned { carrier: 38000, us })
}

/// Capture (wait up to `secs` for a button press) + decode in one shot.
pub fn learn(secs: u64, us_per_sample: f32) -> Result<Learned, String> {
    let raw = i2s::capture(secs, false)?;
    if raw.is_empty() {
        return Err("no IR received (line stayed idle) — aim the remote at the hub's front".into());
    }
    decode(&raw, us_per_sample)
        .ok_or_else(|| "captured a signal but couldn't decode a clean frame".into())
}

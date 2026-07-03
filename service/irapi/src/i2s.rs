//! Direct AR9331 I2S IR-transmit driver — bypasses the stock Logitech `hal` entirely.
//!
//! Reversed from the non-stripped `ath_i2s.ko` + `hal` disassembly (see
//! docs/ir-hardware-reverse-engineering.raw.txt). On this board the IR blaster is driven
//! by clocking a carrier bitstream out of the AR9331 "hornet" I2S peripheral: open
//! `/dev/i2s` O_WRONLY, configure via 4 ioctls, `write()` the MSB-first bit-packed (then
//! 16-bit byte-swapped) waveform, then START (kick TX DMA + set the emitter GPIO enable
//! bits) and DRAIN (block until the ring empties). No cc2544 is needed for transmit.
//!
//! `insmod ath_i2s.ko` already does the pinmux/OE/divider/config bring-up, so we only need
//! the char-device path here. Single-threaded, std + one libc `ioctl` extern (static musl).

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;

#[allow(non_camel_case_types)]
type c_int = i32;
#[allow(non_camel_case_types)]
type c_ulong = u32; // o32 ABI: unsigned long is 32-bit

extern "C" {
    // int ioctl(int fd, unsigned long request, ...); the 3rd arg is a plain int by value.
    fn ioctl(fd: c_int, request: c_ulong, arg: c_int) -> c_int;
}

// ath_i2s.ko ioctls: _IOW('N', nr, int) = 0x80044e2x  (confirmed from the driver switch
// AND hal's call sites — see the hardware RE doc).
pub const I2S_VOLUME: c_ulong = 0x80044e20;
pub const I2S_FREQ: c_ulong = 0x80044e21;
pub const I2S_DSIZE: c_ulong = 0x80044e22;
pub const I2S_MODE: c_ulong = 0x80044e23;
pub const I2S_START: c_ulong = 0x80044e30; // hornet_i2s_write_start: kick TX DMA + GPIO enable
pub const I2S_DRAIN: c_ulong = 0x80044e31; // hornet_i2s_write_complete: block until ring drains

/// I2S data bits per carrier period (hal's `spp` table, keyed on the carrier period in ns).
fn spp(period_ns: u32) -> u32 {
    if period_ns >= 12000 {
        12
    } else if period_ns >= 3000 {
        10
    } else if period_ns >= 2501 {
        8
    } else if period_ns >= 2251 {
        4
    } else {
        2
    }
}

pub const I2S_FREQ_CAP: u32 = 0x0025_8000; // nominal ~666kHz sample clock (1.5µs/sample before the ~16x)

/// IR RECEIVE (learn): oversample the IR-receiver line via /dev/i2s in O_RDONLY. Waits up to
/// `max_secs` for activity (a non-idle chunk), then captures ~300ms more and returns the raw
/// 1-bit-per-sample byte buffer (MSB-first, idle-level = constant 0x00/0xFF bytes). Empty vec =
/// no activity seen. `start_kick` optionally issues I2S_START before reading (RX DMA kick).
pub fn capture(max_secs: u64, start_kick: bool) -> Result<Vec<u8>, String> {
    use std::time::{Duration, Instant};
    let mut f = OpenOptions::new()
        .read(true) // O_RDONLY -> RX ring (NOT the TX ring)
        .open("/dev/i2s")
        .map_err(|e| format!("open /dev/i2s O_RDONLY: {} (hal or a TX blast holding it?)", e))?;
    let fd = f.as_raw_fd();
    unsafe {
        ioc(fd, I2S_DSIZE, 16)?;
        ioc(fd, I2S_MODE, 2)?;
        ioc(fd, I2S_VOLUME, 15)?;
        ioc(fd, I2S_FREQ, I2S_FREQ_CAP as c_int)?;
        if start_kick {
            let _ = ioc(fd, I2S_START, 0); // best-effort RX kick
        }
    }
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut active_at: Option<Instant> = None;
    let mut chunk = [0u8; 192];
    loop {
        let n = match f.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(format!("i2s read: {}", e)),
        };
        let has_activity = chunk[..n].iter().any(|&b| b != 0x00 && b != 0xFF);
        match active_at {
            None => {
                if has_activity {
                    active_at = Some(Instant::now());
                    buf.extend_from_slice(&chunk[..n]);
                }
                // else: idle, keep waiting (discard)
            }
            Some(t0) => {
                buf.extend_from_slice(&chunk[..n]);
                if Instant::now() >= t0 + Duration::from_millis(300) {
                    break;
                }
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        if buf.len() > 512 * 1024 {
            break;
        }
    }
    Ok(buf)
}

/// Run-length decode a raw capture buffer into (is_mark, sample_count) runs. Inverts each byte
/// (hal does `~b` so idle-high reads as 0, mark as 1), MSB-first, skips leading idle.
pub fn rle_decode(raw: &[u8]) -> Vec<(bool, u32)> {
    let mut runs: Vec<(bool, u32)> = Vec::new();
    let (mut cur, mut n, mut started) = (false, 0u32, false);
    for &byte in raw {
        let b = !byte;
        for k in (0..8).rev() {
            let bit = (b >> k) & 1 == 1;
            if !started {
                if !bit {
                    continue;
                }
                started = true;
                cur = true;
                n = 1;
                continue;
            }
            if bit == cur {
                n += 1;
            } else {
                runs.push((cur, n));
                cur = bit;
                n = 1;
            }
        }
    }
    if started && n > 0 {
        runs.push((cur, n));
    }
    runs
}

/// Render a mark/space timing sequence into `(stereo_clk divider, I2S sample bytes)` for a
/// carrier. `seq` = (is_mark, microseconds). Mirrors hal's ir_send renderer: MSB-first bit
/// packing where each carrier period is `on` high bits then `off` low bits for a MARK, or
/// `spp` low bits for a SPACE; finally byte-swap each 16-bit word (the DMA is little-endian).
///
/// `bit_mul` UPSAMPLES the bits-per-carrier-period while keeping the (small, hardware-valid)
/// clock divider unchanged. Our minimal boot's audio reference is ~16x higher than the
/// divider math assumes, so BCLK runs ~16x fast; emitting 16x more bits per period restores
/// the correct real-time carrier WITHOUT pushing the divider out of its usable range (which
/// stalls the serializer). Use bit_mul=1 for the raw hal-equivalent waveform.
pub fn render(carrier_hz: u32, duty: u8, bit_mul: u32, seq: &[(bool, u32)]) -> (u32, Vec<u8>) {
    let period_ns = 1_000_000_000u32 / carrier_hz.max(1);
    let base_spp = spp(period_ns);
    let pulse_bclk_ns = (period_ns + base_spp - 1) / base_spp;
    let stereo_clk = ((pulse_bclk_ns as u64 * 65536) / 40) as u32; // BASE divider (unchanged)
    let s = base_spp * bit_mul.max(1); // upsampled bits per carrier period
    let duty = (duty as u32).clamp(1, 50);
    let on = ((duty * s) + 99) / 100; // ceil(duty*spp/100)
    let off = s - on;

    let mut buf: Vec<u8> = Vec::new();
    let mut cur = 0u8;
    let mut mask = 0x80u8;
    fn push(buf: &mut Vec<u8>, cur: &mut u8, mask: &mut u8, bit: bool, n: u32) {
        for _ in 0..n {
            if bit {
                *cur |= *mask;
            }
            *mask >>= 1;
            if *mask == 0 {
                buf.push(*cur);
                *cur = 0;
                *mask = 0x80;
            }
        }
    }
    for &(mark, us) in seq {
        let cycles = ((us as u64 * 1000 + (period_ns as u64) / 2) / period_ns as u64) as u32;
        for _ in 0..cycles {
            if mark {
                push(&mut buf, &mut cur, &mut mask, true, on);
                push(&mut buf, &mut cur, &mut mask, false, off);
            } else {
                push(&mut buf, &mut cur, &mut mask, false, s);
            }
        }
    }
    if mask != 0x80 {
        buf.push(cur);
    }
    if buf.len() % 2 == 1 {
        buf.push(0);
    }
    for w in buf.chunks_exact_mut(2) {
        w.swap(0, 1);
    }
    (stereo_clk, buf)
}

unsafe fn ioc(fd: c_int, req: c_ulong, arg: c_int) -> Result<(), String> {
    if ioctl(fd, req, arg) < 0 {
        Err(format!(
            "ioctl {:#x} arg {}: {}",
            req,
            arg,
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

/// How a blast finishes. `Drain` calls the blocking DRAIN ioctl (waits for the ring to
/// empty — CORRECT once the clock is known-good, but HANGS the kernel uninterruptibly if a
/// bad divider stalls the serializer). `Hold(ms)` skips DRAIN and just keeps the fd open for
/// `ms` (the DMA plays asynchronously) then closes — a bad divider can never wedge us.
#[derive(Clone, Copy)]
pub enum Finish {
    Drain,
    Hold(u64),
}

/// Blast a pre-rendered sample buffer out /dev/i2s. `select` = the raw emitter selector
/// passed in the high 16 bits of the START ioctl arg (bit0->GPIO16, bit1->GPIO28,
/// bit2->GPIO13; the kernel does the bit->GPIO translation).
pub fn blast_samples(
    stereo_clk: u32,
    select: u32,
    samples: &[u8],
    finish: Finish,
) -> Result<(), String> {
    let f = OpenOptions::new()
        .write(true) // O_WRONLY -> the 512-entry TX ring (works for both driver open-mode checks)
        .open("/dev/i2s")
        .map_err(|e| format!("open /dev/i2s: {} (is hal still holding it? stop hal first)", e))?;
    let fd = f.as_raw_fd();
    unsafe {
        ioc(fd, I2S_DSIZE, 16)?;
        ioc(fd, I2S_MODE, 2)?;
        ioc(fd, I2S_VOLUME, 15)?;
        ioc(fd, I2S_FREQ, stereo_clk as c_int)?;
    }
    (&f).write_all(samples).map_err(|e| format!("i2s write: {}", e))?;
    // START arg = (select << 16) | (len & 0xffff); the kernel reads select = arg>>16 and
    // sets GPIO_OUT bits from its low 3 bits. NOTE: pass the raw 0..7 selector, NOT GPIO bits.
    let start_arg = (((select & 0xffff) << 16) | (samples.len() as u32 & 0xffff)) as c_int;
    unsafe {
        ioc(fd, I2S_START, start_arg)?;
    }
    match finish {
        Finish::Drain => unsafe { ioc(fd, I2S_DRAIN, 0)? },
        Finish::Hold(ms) => std::thread::sleep(std::time::Duration::from_millis(ms)),
    }
    Ok(()) // fd closes on drop (release resets the DMA)
}

/// Render + blast a mark/space sequence at `carrier_hz` on the emitters in `select` (0..7).
pub fn blast(carrier_hz: u32, duty: u8, bit_mul: u32, select: u32, seq: &[(bool, u32)], finish: Finish) -> Result<(), String> {
    let (stereo_clk, samples) = render(carrier_hz, duty, bit_mul, seq);
    blast_samples(stereo_clk, select, &samples, finish)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_mark_bit_pattern() {
        // 38 kHz -> period 26315 ns, spp 12, duty 50 -> on 6 / off 6 => period pattern
        // 111111000000. us=53 rounds to 2 carrier periods = 24 bits = FC 0F C0 (pre-swap),
        // padded to 4 bytes then 16-bit byte-swapped -> 0F FC 00 C0.
        let (_clk, buf) = render(38000, 50, 1, &[(true, 53)]);
        assert_eq!(buf, vec![0x0F, 0xFC, 0x00, 0xC0]);
    }

}

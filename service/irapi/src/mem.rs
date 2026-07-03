//! Minimal /dev/mem MMIO peek/poke for AR9331 hardware debugging. Lets us read the I2S
//! clock/config and GPIO enable registers directly to prove whether the IR signal path is
//! actually being driven (objective, no camera). Single-threaded, std + libc mmap extern.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

#[allow(non_camel_case_types)]
type c_int = i32;

extern "C" {
    // void *mmap(void*, size_t, int prot, int flags, int fd, off_t off);
    // musl uses a 64-bit off_t on ALL arches (LFS-always), so offset MUST be i64.
    fn mmap(addr: usize, length: usize, prot: c_int, flags: c_int, fd: c_int, offset: i64) -> isize;
    fn munmap(addr: usize, length: usize) -> c_int;
}
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const PAGE: usize = 0x1000;
const MAP_FAILED: isize = -1;

/// Accept a KSEG1 virtual (0xbXXXXXXX) or a physical (0x1XXXXXXX) address -> physical.
fn phys(addr: u32) -> u32 {
    addr & 0x1FFF_FFFF
}

fn with_reg<T>(addr: u32, f: impl FnOnce(*mut u8) -> T) -> Result<T, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/mem")
        .map_err(|e| format!("open /dev/mem: {}", e))?;
    let p = phys(addr) as usize;
    let base = p & !(PAGE - 1);
    let off = p & (PAGE - 1);
    let m = unsafe {
        mmap(0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, file.as_raw_fd(), base as i64)
    };
    if m == MAP_FAILED {
        return Err(format!("mmap {:#x}: {}", base, std::io::Error::last_os_error()));
    }
    let r = f(unsafe { (m as *mut u8).add(off) });
    unsafe {
        munmap(m as usize, PAGE);
    }
    Ok(r)
}

pub fn peek(addr: u32) -> Result<u32, String> {
    with_reg(addr, |p| unsafe { std::ptr::read_volatile(p as *const u32) })
}

pub fn poke(addr: u32, val: u32) -> Result<(), String> {
    with_reg(addr, |p| unsafe { std::ptr::write_volatile(p as *mut u32, val) })
}

/// The AR9331 registers that matter for IR TX, with expected values (from the ath_i2s.ko
/// bring-up reversed in docs/ir-hardware-reverse-engineering.raw.txt).
pub const IR_REGS: &[(u32, &str, &str)] = &[
    (0xb80b0000, "STEREO_CONFIG", "insmod=0x00a21302 |0x80000 reset -> ~0x00a29302"),
    (0xb80b0004, "STEREO_VOLUME", "hal VOLUME=15 -> 0x0 (max)"),
    (0xb80b001c, "STEREO_CLKDIV", "insmod=0x00215999; hal rewrites per-carrier"),
    (0xb8040000, "GPIO_OE       ", "expect bits 12,13,16,28 set"),
    (0xb8040008, "GPIO_OUT      ", "emitter-enable bits set by I2S_START"),
    (0xb8040004, "GPIO_IN       ", ""),
    (0xb8040028, "GPIO_FUNCTION ", "expect 0x24000000 (I2S mux, bits 26,29)"),
    (0xb80a001c, "MBOX_TX_CTRL  ", "TX DMA control (1=pause,2=start,4=resume)"),
    (0xb80a0044, "MBOX_IRQ_STAT ", "bit 0x400 = TX done"),
];

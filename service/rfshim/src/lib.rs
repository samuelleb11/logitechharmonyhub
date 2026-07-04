//! rfshim — a tiny LD_PRELOAD interposer to CAPTURE what the stock Logitech `hal` writes to
//! /dev/rfspi (the cc2544 radio command channel). no_std, no libc: raw MIPS o32 syscalls only, so
//! uClibc's loader can preload it into hal. Every write() to /dev/rfspi (plus any short binary
//! write that looks like a cc2544 command) is logged as hex to /tmp/rf_hal.log.
//!
//!   LD_PRELOAD=/root/rfshim.so /usr/bin/hal -f -s   # (with dbus + lo up)
//!
//! Then reproduce the captured init/pairing command sequence in irapi's rf.rs and drop hal.
#![no_std]
#![feature(asm_experimental_arch)]

use core::arch::global_asm;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

// MIPS o32 syscall: C args (nr,a0,a1,a2) arrive in $a0-$a3. Put nr in $v0, shift args to $a0-$a2,
// `syscall`, return $v0. Leaf, no $gp use.
global_asm!(
    r#"
.set noreorder
.globl __sc3
.type __sc3,@function
__sc3:
    move $2, $4
    move $4, $5
    move $5, $6
    move $6, $7
    syscall
    jr $31
    nop
"#
);

extern "C" {
    fn __sc3(nr: i32, a0: i32, a1: i32, a2: i32) -> i32;
}

const NR_READ: i32 = 4003;
const NR_WRITE: i32 = 4004;
const NR_OPEN: i32 = 4005;

// track up to 4 fds that were opened as /dev/rfspi (hal opens it more than once)
static mut RFSPI_FDS: [i32; 4] = [-3, -3, -3, -3];
static mut LOG_FD: i32 = -3;

const HEX: &[u8; 16] = b"0123456789abcdef";

unsafe fn is_rfspi_fd(fd: i32) -> bool {
    let mut i = 0;
    while i < 4 {
        if RFSPI_FDS[i] == fd {
            return true;
        }
        i += 1;
    }
    false
}
unsafe fn add_rfspi_fd(fd: i32) {
    let mut i = 0;
    while i < 4 {
        if RFSPI_FDS[i] < 0 {
            RFSPI_FDS[i] = fd;
            return;
        }
        i += 1;
    }
    RFSPI_FDS[0] = fd;
}

unsafe fn sys_read(fd: i32, buf: *mut u8, n: i32) -> i32 {
    __sc3(NR_READ, fd, buf as i32, n)
}
unsafe fn sys_write(fd: i32, buf: *const u8, n: i32) -> i32 {
    __sc3(NR_WRITE, fd, buf as i32, n)
}
unsafe fn sys_open(path: *const u8, flags: i32, mode: i32) -> i32 {
    __sc3(NR_OPEN, path as i32, flags, mode)
}

unsafe fn ensure_log() {
    if LOG_FD < 0 {
        // MIPS flags: O_WRONLY(0x1)|O_CREAT(0x100)|O_APPEND(0x8) = 0x109, mode 0644
        LOG_FD = sys_open(b"/tmp/rf_hal.log\0".as_ptr(), 0x109, 0o644);
    }
}

unsafe fn is_rfspi(path: *const u8) -> bool {
    let want = b"/dev/rfspi";
    let mut i = 0;
    while i < want.len() {
        if *path.add(i) != want[i] {
            return false;
        }
        i += 1;
    }
    *path.add(want.len()) == 0
}

fn itoa(out: &mut [u8], mut v: i32) -> usize {
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let neg = v < 0;
    if neg {
        v = -v;
    }
    let mut tmp = [0u8; 12];
    let mut n = 0;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut p = 0;
    if neg {
        out[p] = b'-';
        p += 1;
    }
    for i in (0..n).rev() {
        out[p] = tmp[i];
        p += 1;
    }
    p
}

unsafe fn log_frame(tag: u8, fd: i32, buf: *const u8, n: i32) {
    ensure_log();
    let mut line = [0u8; 256];
    let mut p = 0usize;
    line[p] = tag;
    p += 1;
    line[p] = b' ';
    p += 1;
    p += itoa(&mut line[p..], fd);
    line[p] = b' ';
    p += 1;
    p += itoa(&mut line[p..], n);
    line[p] = b':';
    p += 1;
    let cap = if n > 60 { 60 } else { n };
    let mut i = 0;
    while i < cap {
        line[p] = b' ';
        p += 1;
        let byte = *buf.add(i as usize);
        line[p] = HEX[(byte >> 4) as usize];
        p += 1;
        line[p] = HEX[(byte & 0xf) as usize];
        p += 1;
        i += 1;
    }
    line[p] = b'\n';
    p += 1;
    let _ = sys_write(LOG_FD, line.as_ptr(), p as i32);
}

#[no_mangle]
pub extern "C" fn open(path: *const u8, flags: i32, mode: i32) -> i32 {
    let fd = unsafe { sys_open(path, flags, mode) };
    if fd >= 0 && unsafe { is_rfspi(path) } {
        unsafe {
            add_rfspi_fd(fd);
            log_frame(b'O', fd, b"".as_ptr(), 0);
        }
    }
    fd
}

#[no_mangle]
pub extern "C" fn open64(path: *const u8, flags: i32, mode: i32) -> i32 {
    open(path, flags, mode)
}

#[no_mangle]
pub extern "C" fn write(fd: i32, buf: *const u8, n: usize) -> isize {
    unsafe {
        let ni = n as i32;
        let first = if ni > 0 { *buf } else { 0 };
        let looks_cmd = ni > 0 && ni <= 40 && (0x10..=0x30).contains(&first);
        if is_rfspi_fd(fd) || looks_cmd {
            log_frame(b'W', fd, buf, ni);
        }
        sys_write(fd, buf, ni) as isize
    }
}

#[no_mangle]
pub extern "C" fn read(fd: i32, buf: *mut u8, n: usize) -> isize {
    unsafe {
        let r = sys_read(fd, buf, n as i32);
        if r > 0 && is_rfspi_fd(fd) {
            log_frame(b'R', fd, buf as *const u8, r);
        }
        r as isize
    }
}

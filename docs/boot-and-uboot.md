# Boot Flow & U-Boot

## Boot chain
```
U-Boot 1.1.4 (ar7240>)  --bootcmd-->  bootm 0x9f010000 ; bootm 0x9f100000
   0x9f010000 = "kernel"  (primary)
   0x9f100000 = "kernel2" (fallback)
        |
        v
Linux 2.6.31 (lzma kernel "Pimento Kernel Image")
        |
        v
init  (= /sbin/init by default)  ->  BusyBox  ->  /etc/init.d/rcS  ->  Harmony app runtime
```

## Stock U-Boot environment (`printenv`)
```
bootargs=console=none init=/sbin/init
bootcmd=bootm 0x9f010000 ; bootm 0x9f100000
bootdelay=0
baudrate=115200
ethaddr=0x00:0xaa:0xbb:0xcc:0xdd:0xee
ipaddr=192.168.1.2
serverip=192.168.1.10
stdin=serial
stdout=serial
stderr=serial
ethact=eth0
Environment size: 242/65532 bytes
```
Notes:
- `console=none` is why a **normal boot prints no kernel/userspace text**.
- `bootdelay=0` → autoboot is immediate; flood Enter to interrupt (see uart doc).
- U-Boot prompt is `ar7240>`. Network defaults: device `192.168.1.2`, tftp server `192.168.1.10`.

## Getting a readable VERBOSE boot (RAM-only, reverts on power cycle)
Interrupt autoboot, then:
```
setenv bootargs console=ttyS0,115200 init=/sbin/init
bootm 0x9f010000
```
Full kernel cmdline that results:
```
console=ttyS0,115200 init=/sbin/init mem=32M \
  mtdparts=ar7240-nor0:64k(u-boot),960k(kernel),960k(kernel2),4096k(root),5120k(data),5120k(cache),64k(mfg)
```
Console text is now readable, but **still interleaved with the app's binary stream**
because `init=/sbin/init` launches the full Harmony runtime.

## Getting a clean ROOT SHELL (recommended for exploration)
Boot the shell directly instead of the app — this stops the binary pollution:
```
setenv bootargs console=ttyS0,115200 init=/bin/sh
bootm 0x9f010000
```
(If `/bin/sh` is missing, try `init=/bin/ash` or `init=/bin/busybox sh`.)
Status as of 2026-06-21: **not yet attempted** — try this next.

> ⚠️ Do **not** run `saveenv` unless you intend to make a change permanent. All the
> overrides above are RAM-only and revert on the next power cycle, which keeps the
> device recoverable.

## Useful next steps once at a shell
- `cat /proc/mtd`, `cat /proc/cmdline`, `mount`, `ps`, `cat /proc/version`
- Dump partitions for offline analysis: `cat /dev/mtd3 > /tmp/root.bin` then transfer
  (tftp to `192.168.1.10`, or over the console with a chunked encoder).
- Inspect `/etc/init.d/rcS` and the app launcher to understand the runtime.

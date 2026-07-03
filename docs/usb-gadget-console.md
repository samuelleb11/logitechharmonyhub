# USB Gadget — console / network without soldering

Goal: use the hub's **USB port** (a peripheral/gadget port — the same one the stock
app uses to talk to MyHarmony) as a console and/or network transport, so you can
plug the hub into a computer and get a shell without soldering to the UART pads.

## Substrate validated (2026-06-21)
Loading the stock gadget modules works end to end:

```
insmod /lib/modules/2.6.31-g89d565c/kernel/drivers/usb/gadget/ath_udc.ko   # UDC: "ath_usb_init: id: 111"
insmod /lib/modules/2.6.31-g89d565c/kernel/drivers/usb/gadget/gadgetfs.ko  # "USB Gadget filesystem"
mkdir -p /dev/gadget && mount -t gadgetfs none /dev/gadget
ls /dev/gadget    # -> ath_udc   (the gadgetfs ep0 control endpoint)
```

`/dev/gadget/ath_udc` is the control endpoint a userspace program uses to present
**any** USB class. (`unable to bind driver nop --> -139` on mount is normal.)
Modules are RAM-only — nothing auto-loads at `init=/bin/sh`, all gone on reboot.

## The constraint
Stock firmware ships **only** `gadgetfs.ko` + `ath_udc.ko` — there is **no
`g_serial.ko` / `g_ether.ko`**, and `g_serial` is **not** built into the kernel
(gadgetfs successfully bound the UDC, which it could not if a serial/ether gadget had
claimed it). There is no `/proc/config.gz` and no `/sys/class/udc` (kernel too old).
The host enumerates nothing until a userspace gadgetfs program writes descriptors.

So this is not "modprobe and done" — it needs a small amount of build work.

## Options (ranked; A/B keep the stock kernel and the cc2544 IR driver)

| | Approach | Keeps IR | Effort |
|---|---|---|---|
| **A ⭐** | **gadgetfs userspace CDC-ACM**: a program opens `/dev/gadget/ath_udc`, writes ACM descriptors (1 interrupt + 2 bulk eps), bridges the bulk data to a getty/shell. Prior art: David Brownell's gadgetfs example. Cross-compiled static MIPS-BE binary, run from init. | ✅ | medium |
| **B** | Build stock `g_serial.ko` / `g_ether.ko` from **Logitech's GPL kernel source** (2.6.31) → `insmod g_serial` → `/dev/ttyGS0` → getty. | ✅ | low *if* the source is obtainable |
| **C** | OpenWrt (`kmod-usb-gadget-serial` is trivial there). | ❌ loses cc2544 IR | high + defeats the project |

**Decision:** start with **A** (a quick win that touches nothing else), keep **B** as
the cleaner end state if the GPL source builds.

## The upgrade worth aiming for
Instead of a *serial* gadget, a **USB-Ethernet gadget** (`g_ether`, or a gadgetfs
ECM implementation) turns the one USB cable into a network link → run `dropbear`
(ssh console) **and** serve the HTTP/gRPC IR API over the same cable. One plug =
power + console + API, no WiFi setup. More work than ACM (RNDIS/ECM descriptors),
so the sequence is: **ACM serial console first, graduate to USB-ethernet** once the
IR service exists. See [ir-api-project.md](ir-api-project.md).

#!/bin/sh
# Standalone IR bring-up for Logitech Harmony Hub (AR9331), NO luaworks / NO cloud.
# Goal: cc2544 IR ready + /usr/bin/hal serving LTCP on 127.0.0.1:16716, watchdog fed.
# Run this from the jffs2 overlay (e.g. /data) instead of the stock rcS app section.
# Assumes the stock rootfs is mounted normally (squashfs+jffs2 unionfs), /dev nodes present.
#
# VERIFIED WORKING 2026-07-02 (from init=/bin/sh): dbus + `ifconfig lo up` + `hal -f -s`
# -> hal listens on 127.0.0.1:16716. The loopback bring-up is CRITICAL: hal binds its LTCP
# listener to 127.0.0.1, and without lo up it dies with "libhal_run: ERROR on binding:
# Cannot assign requested address" (EADDRNOTAVAIL). BT/wlan/i2s startup errors are non-fatal.
#
# (no `set -e`: each stage is logged and non-fatal so a failure is debuggable, never bricking)

KREL=$(uname -r)                 # 2.6.31-g89d565c
MODDIR=/lib/modules/$KREL
LOG() { echo "[bringup] $*"; }

# --- 0. volatile dirs the stock rcS makes; hal + watchdog need /var/watchdog ---
for d in cache lock log run tmp watchdog dbus; do
    [ -d /var/volatile/$d ] || mkdir -p /var/volatile/$d
done
# /var/watchdog is a symlink -> volatile/watchdog in the stock rootfs; ensure it resolves.
[ -d /var/watchdog ] || mkdir -p /var/watchdog

# memory overcommit (stock rcS.local: avoids OOM-kill -> watchdog reboot on 32MB box)
echo 1 > /proc/sys/vm/overcommit_memory 2>/dev/null || true

# --- 0b. loopback — CRITICAL: hal binds its LTCP listener to 127.0.0.1 ---
# Stock rcS does `ifup lo`; under init=/bin/sh it must be done explicitly or hal aborts.
LOG "bringing up loopback"
/sbin/ifconfig lo 127.0.0.1 netmask 255.0.0.0 up 2>/dev/null || /sbin/ifup lo 2>/dev/null || true

# --- 0c. tmpfs on /var/volatile so /tmp is RAM-backed (not the jffs2 overlay) ---
# Under init=/bin/sh, `mount -a` didn't run, so /tmp writes hit the persistent ~5MB overlay
# and can fill it. Mount a tmpfs to match stock behavior. (Skip if already a mountpoint.)
if ! grep -q ' /var/volatile ' /proc/mounts 2>/dev/null; then
    LOG "mounting tmpfs on /var/volatile"
    mount -t tmpfs -o size=8M tmpfs /var/volatile 2>/dev/null || true
    for d in cache lock log run tmp watchdog dbus; do mkdir -p /var/volatile/$d; done
fi

# --- 1. logging (optional but hal -s logs to syslog) ---
[ -e /var/run/syslogd.pid ] || /sbin/syslogd -C256 2>/dev/null || true
/sbin/klogd 2>/dev/null || true

# --- 2. IR module + firmware ------------------------------------------------
# cc2544.ko has depends= (none); uses built-in ar7240_gpio_*/ar7240_flash_spi_* kernel syms.
# Loading it alone is enough for IR. /dev/rfspi (10,59) and /dev/rffw (10,60) are static nodes.
if ! grep -q '^cc2544 ' /proc/modules 2>/dev/null; then
    LOG "insmod cc2544.ko"
    /sbin/insmod $MODDIR/cc2544.ko
fi
# NOTE: hal will (re)load the firmware itself by comparing /lib/firmware/cc2544.version to the
# chip and running `cat /lib/firmware/cc2544.bin > /dev/rffw` via system() if they differ.
# We pre-load it so the chip is ready even before hal, and so hal sees a matching version:
if [ -c /dev/rffw ]; then
    LOG "loading cc2544 firmware into chip RAM"
    cat /lib/firmware/cc2544.bin > /dev/rffw
fi

# --- 3. dbus (system bus) --------------------------------------------------
# hal links libdbus and calls dbus_bus_get() at startup (for org.bluez only). To be safe and
# match stock ordering (dbus is started BEFORE hal in rcS), bring up the system bus. Cheap.
if [ -x /usr/bin/dbus-daemon ]; then
    mkdir -p /var/run/dbus
    /usr/bin/dbus-uuidgen --ensure 2>/dev/null || true
    if [ ! -e /var/run/dbus/system_bus_socket ]; then
        LOG "starting dbus-daemon --system"
        /usr/bin/dbus-daemon --system
    fi
fi

# --- 4. watchdog daemon ----------------------------------------------------
# /usr/bin/watchdog: opens /dev/watchdog, WDIOC_SETTIMEOUT=15s, every 7s scans /var/watchdog/*;
# if ANY file's mtime is older than (now-15s) it stops kicking -> hardware reboots in <=15s.
# hal's own watchdog thread touches /var/watchdog/hal (open O_WRONLY|O_CREAT 0644 + close) on a
# timer. So: as long as hal is healthy, the chain hal->/var/watchdog/hal->watchdog->/dev/watchdog
# keeps the box alive. Start hal FIRST (so /var/watchdog/hal exists) then the watchdog daemon, OR
# pre-create the file. We pre-create then start both:
: > /var/watchdog/hal           # seed so watchdog doesn't see a stale/absent monitored file
export LD_PRELOAD=/lib/libcrashlog.so   # stock rcS sets this for crash reporting (optional)
if [ -x /usr/bin/watchdog ] && [ ! -e /etc/nowatchdog ]; then
    LOG "starting hardware watchdog feeder"
    /usr/bin/watchdog
fi

# --- 5. hal ----------------------------------------------------------------
# Flags: -f (don't fork/daemonize), -s (syslog), -d (debug), -w (disable hal's watchdog thread),
#        -t (RF firmware test then exit). Stock uses: hal -f -s
# Run it backgrounded here so the script can continue. Keep -f so we control the process.
# Do NOT pass -w in production: that disables the thread that touches /var/watchdog/hal, which
# would make the watchdog daemon reboot the box. (Use -w only when watchdog daemon is NOT running.)
LOG "starting /usr/bin/hal -f -s (LTCP on 127.0.0.1:16716)"
/usr/bin/hal -f -s &                 # VERIFIED: -f -s (stock flags). -f = don't daemonize.
HAL_PID=$!
LOG "hal pid=$HAL_PID"

# Wait for hal to bind 16716 before declaring IR ready (best-effort).
# hal binds 127.0.0.1 -> /proc/net/tcp shows 7F000001:414C (NOT 00000000).
i=0
while [ $i -lt 50 ]; do
    if grep -qi '7F000001:414C' /proc/net/tcp 2>/dev/null; then   # 127.0.0.1:16716
        LOG "hal is listening on 127.0.0.1:16716"; break
    fi
    i=$((i+1)); sleep 1 || break
done

# --- 6. Wi-Fi station (untethered access) ----------------------------------
# Needs credentials in /etc/wifi/wpa_supplicant.conf (copy wpa_supplicant.conf.example,
# fill SSID + PSK, chmod 600). Interface tiers: wifi0 (radio) -> ath0 (station VAP).
# wpa_supplicant on this build only has wext/madwifi backends (NO nl80211).
WIFICONF=${WIFICONF:-/etc/wifi/wpa_supplicant.conf}
if [ -r "$WIFICONF" ]; then
    LM=/lib/modules/$KREL
    LOG "loading Atheros wifi stack"
    for m in asf adf ath_hal ath_rate_atheros ath_dev umac; do
        [ -d /sys/module/$m ] || /sbin/insmod $LM/$m.ko 2>/dev/null
    done
    i=0; while [ ! -d /sys/class/net/wifi0 ] && [ $i -lt 50 ]; do usleep 100000 2>/dev/null || sleep 1; i=$((i+1)); done
    [ -d /sys/class/net/ath0 ] || /sbin/wlanconfig ath0 create wlandev wifi0 wlanmode sta
    /sbin/ifconfig ath0 up
    mkdir -p /var/run/wpa_supplicant
    LOG "starting wpa_supplicant (wext) on ath0"
    /usr/sbin/wpa_supplicant -s -B -Dwext -iath0 -c "$WIFICONF"
    # wait for association, then DHCP
    i=0; while [ $i -lt 30 ]; do /usr/sbin/wpa_cli -i ath0 status 2>/dev/null | grep -q 'wpa_state=COMPLETED' && break; sleep 1; i=$((i+1)); done
    LOG "DHCP on ath0"
    /sbin/udhcpc -i ath0 -b -S -t 5 -T 2 -A 20 -p /var/run/udhcpc.ath0.pid -s /usr/share/udhcpc/default.script
    /sbin/ifconfig ath0 | grep 'inet addr' | sed 's/^/[bringup] ath0 /'
else
    LOG "no $WIFICONF -> skipping Wi-Fi (still reachable over UART)"
fi

# --- 7. dropbear SSH (untethered dev shell) --------------------------------
# Device ships dropbear; stock rcS gates it on /etc/tdeenable. Use key auth via
# /root/.ssh/authorized_keys (deploy service/ssh/authorized_keys there).
if [ -x /usr/sbin/dropbear ]; then
    mkdir -p /etc/dropbear /root/.ssh
    chmod 700 /root/.ssh 2>/dev/null || true
    [ -f /etc/dropbear/dropbear_rsa_host_key ] || /usr/sbin/dropbearkey -t rsa -f /etc/dropbear/dropbear_rsa_host_key 2>/dev/null
    LOG "starting dropbear (ssh) on :22"
    /usr/sbin/dropbear -R 2>/dev/null || /usr/sbin/dropbear 2>/dev/null
fi

# --- 8. our Rust IR service (once built) -----------------------------------
# /mnt/data/irapi/irapi serve --config /mnt/data/irapi/config.json &   # (M2+)

LOG "bring-up complete; IR available via hal LTCP /ir/ir_send and /ir/ir_cap"
LOG "if wifi up: ssh -i service/ssh/harmony_id_rsa root@<ath0-ip>"
wait $HAL_PID

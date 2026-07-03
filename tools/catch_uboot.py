#!/usr/bin/env python3
"""Flood Enter into the UART to interrupt U-Boot autoboot, then stop.

Holds the command FIFO open and writes RAW carriage returns every ~12ms while
watching the live log. When the U-Boot autoboot countdown appears it floods a
little longer to guarantee a keypress lands in the window, then stops and reports
whether it reached a prompt. Designed to run in the background while the user
power-cycles the device.
"""
import os, sys, json, time, base64

st = json.load(open("/tmp/uartctl.state.json"))
fifo = st["fifo"]; log = st["log"]
MAXSEC = float(sys.argv[1]) if len(sys.argv) > 1 else 120.0

RAW_CR = b"RAW " + base64.b64encode(b"\r") + b"\n"

logf = open(log, "rb")
logf.seek(0, os.SEEK_END)   # only watch NEW output

# open FIFO (daemon is the reader); non-blocking retry until daemon is there
import errno
fd = None
deadline = time.time() + 10
while time.time() < deadline:
    try:
        fd = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
        break
    except OSError as e:
        if e.errno in (errno.ENXIO, errno.ENOENT):
            time.sleep(0.1); continue
        raise
if fd is None:
    print("CATCH: could not open FIFO"); sys.exit(1)
ff = os.fdopen(fd, "wb", buffering=0)

buf = b""
saw_countdown = False
extra_until = None
start = time.time()
result = "TIMEOUT"

while time.time() - start < MAXSEC:
    try:
        ff.write(RAW_CR)
    except BrokenPipeError:
        # daemon reopened FIFO; reopen
        try:
            ff.close()
            fd = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
            ff = os.fdopen(fd, "wb", buffering=0)
        except OSError:
            time.sleep(0.1)
    new = logf.read()
    if new:
        buf += new
    if not saw_countdown and (b"stop autoboot" in buf or b"Hit any key" in buf):
        saw_countdown = True
        extra_until = time.time() + 0.6   # keep flooding briefly to land a key
    if saw_countdown and time.time() >= extra_until:
        result = "CAUGHT"
        break
    time.sleep(0.012)

# settle: stop flooding, send a couple clean newlines, read prompt area
time.sleep(0.3)
buf += logf.read()
ff.close()

# heuristic: did we reach a U-Boot prompt rather than booting the kernel?
booted = (b"Booting image" in buf) or (b"Transferring control to Linux" in buf)
tail = buf[-400:].decode("latin-1", "replace")
print("CATCH RESULT:", result)
print("saw_countdown:", saw_countdown, " kernel_booted:", booted)
print("---- tail of new output ----")
print(tail)

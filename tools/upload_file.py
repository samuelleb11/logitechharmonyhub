#!/usr/bin/env python3
"""Reliably upload a TEXT-SAFE file (e.g. base64) to the device through the
uartctl daemon, by APPENDING it in small chunks with a per-chunk handshake.

Why not just stream into `cat >`: the device tty is in cooked/canonical mode
with echo, whose input buffer is ~4096 bytes. Streaming faster than it drains
silently drops everything past one buffer. So we send one `printf ... >> file`
command per chunk (each command line stays under the canonical limit) and wait
for a unique sentinel before sending the next — deterministic, no timing guess.

Input must be ASCII with no single quotes (base64 qualifies). Verify with md5
afterward regardless.

Usage: upload_file.py <localfile> <devpath> [chunkchars]
"""
import os, sys, json, base64, time

st = json.load(open("/tmp/uartctl.state.json"))
fifo, logp = st["fifo"], st["log"]
local, devpath = sys.argv[1], sys.argv[2]
chunk = int(sys.argv[3]) if len(sys.argv) > 3 else 2000

_ctr = 0
def fifo_put(line):
    fd = os.open(fifo, os.O_WRONLY)
    with os.fdopen(fd, "wb") as f:
        f.write(line + b"\n")

def send_cmd_wait(cmd, timeout=20):
    global _ctr
    _ctr += 1
    sent = "__UP_%d_%d__" % (os.getpid(), _ctr)
    start = os.path.getsize(logp)
    fifo_put(b"TXT " + base64.b64encode((cmd + " ; echo " + sent).encode()))
    needle = ("\n" + sent).encode()
    deadline = time.time() + timeout
    buf = b""
    with open(logp, "rb") as f:
        f.seek(start)
        while time.time() < deadline:
            c = f.read()
            if c:
                buf += c
                if needle in buf:
                    return True
            else:
                time.sleep(0.02)
    return False

with open(local, "rb") as fh:
    data = fh.read()
text = data.decode("ascii").replace("\n", "").replace("\r", "")  # base64 body, one stream

if not send_cmd_wait(": > %s" % devpath):
    sys.exit("reset of %s failed" % devpath)

n = len(text)
for i in range(0, n, chunk):
    piece = text[i:i + chunk]
    # base64 is shell-safe UNQUOTED as a printf ARG (no * ? [ space ' " etc.),
    # so a truncated line can never leave an open quote and wedge the shell.
    if not send_cmd_wait("printf %%s %s >> %s" % (piece, devpath)):
        sys.exit("\nchunk at %d FAILED (canonical limit? retry smaller chunk)" % i)
    if (i // chunk) % 15 == 0:
        sys.stderr.write("\rupload %d/%d chars (%d%%)" % (i + len(piece), n, (i + len(piece)) * 100 // n))
        sys.stderr.flush()
sys.stderr.write("\rupload done: %d base64 chars -> %s\n" % (n, devpath))

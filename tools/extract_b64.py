#!/usr/bin/env python3
"""Extract a base64 block emitted by dumpflash.lua from a UART log and verify it.

Finds the LAST complete  ===B64BEGIN <path>=== ... ===B64END <path> ...===
block in the log, base64-decodes the body, writes it to <outfile>, and prints
the decoded size + md5 so it can be compared with the device's own md5sum.

Usage: extract_b64.py <logfile> <device-path> <outfile>
  e.g. extract_b64.py /tmp/uart-latest.log /dev/mtd6 backups/mtd6-mfg.bin
"""
import sys, re, base64, hashlib, os

def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    logfile, path, outfile = sys.argv[1], sys.argv[2], sys.argv[3]
    with open(logfile, "rb") as f:
        data = f.read()
    # Normalise CRLF/CR -> LF so console line-discipline translation is undone.
    data = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    begin = ("===B64BEGIN %s===" % path).encode()
    end_re = re.compile((r"===B64END %s bytes=(\d+)===" % re.escape(path)).encode())

    # take the LAST begin marker (most recent attempt)
    bpos = data.rfind(begin)
    if bpos == -1:
        sys.exit("no B64BEGIN marker for %s in %s" % (path, logfile))
    after = data[bpos + len(begin):]
    m = end_re.search(after)
    if not m:
        sys.exit("found BEGIN but no matching B64END for %s (dump incomplete?)" % path)
    claimed = int(m.group(1))
    body = after[:m.start()]
    # keep only base64 alphabet chars
    b64 = bytes(c for c in body if c in
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=")
    raw = base64.b64decode(b64)
    os.makedirs(os.path.dirname(os.path.abspath(outfile)), exist_ok=True)
    with open(outfile, "wb") as f:
        f.write(raw)
    md5 = hashlib.md5(raw).hexdigest()
    ok = (len(raw) == claimed)
    print("path        : %s" % path)
    print("outfile     : %s" % outfile)
    print("decoded     : %d bytes" % len(raw))
    print("device says : %d bytes  (%s)" % (claimed, "len OK" if ok else "LEN MISMATCH!"))
    print("md5         : %s" % md5)
    if not ok:
        sys.exit(3)

if __name__ == "__main__":
    main()

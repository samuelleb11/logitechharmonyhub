#!/usr/bin/env python3
"""Extract + decode an RLE64 block produced by dumpflash_rle.lua and verify it.

Finds the LAST  ===RLE64BEGIN <path>=== ... ===RLE64END <path> rawbytes=N===
block in the UART log, base64-decodes the body to the RLE token stream, expands
the tokens to raw bytes, writes <outfile>, and prints size + md5 for comparison
with the device's own md5sum.

RLE tokens:
  0x00 LENhi LENlo  <LEN literal bytes>
  0x01 BYTE  C4 C3 C2 C1   (BYTE repeated, 32-bit big-endian count)

Usage: extract_rle64.py <logfile> <device-path> <outfile>
"""
import sys, re, base64, hashlib, os

def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    logfile, path, outfile = sys.argv[1], sys.argv[2], sys.argv[3]
    with open(logfile, "rb") as f:
        data = f.read()
    data = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    begin = ("===RLE64BEGIN %s===" % path).encode()
    end_re = re.compile((r"===RLE64END %s rawbytes=(\d+)===" % re.escape(path)).encode())

    bpos = data.rfind(begin)
    if bpos == -1:
        sys.exit("no RLE64BEGIN marker for %s in %s" % (path, logfile))
    after = data[bpos + len(begin):]
    m = end_re.search(after)
    if not m:
        sys.exit("found BEGIN but no matching RLE64END for %s (dump incomplete?)" % path)
    claimed = int(m.group(1))
    body = after[:m.start()]
    b64 = bytes(c for c in body if c in
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=")
    tok = base64.b64decode(b64)

    # expand RLE token stream
    raw = bytearray()
    i, n = 0, len(tok)
    while i < n:
        op = tok[i]; i += 1
        if op == 0x00:
            if i + 2 > n: sys.exit("truncated literal header at %d" % i)
            ln = (tok[i] << 8) | tok[i + 1]; i += 2
            if i + ln > n: sys.exit("truncated literal body (need %d) at %d" % (ln, i))
            raw += tok[i:i + ln]; i += ln
        elif op == 0x01:
            if i + 5 > n: sys.exit("truncated run token at %d" % i)
            b = tok[i]
            cnt = (tok[i + 1] << 24) | (tok[i + 2] << 16) | (tok[i + 3] << 8) | tok[i + 4]
            i += 5
            raw += bytes([b]) * cnt
        else:
            sys.exit("bad RLE opcode 0x%02x at %d" % (op, i - 1))

    raw = bytes(raw)
    os.makedirs(os.path.dirname(os.path.abspath(outfile)), exist_ok=True)
    with open(outfile, "wb") as f:
        f.write(raw)
    md5 = hashlib.md5(raw).hexdigest()
    ok = (len(raw) == claimed)
    ratio = (len(b64) / len(raw)) if raw else 0
    print("path        : %s" % path)
    print("outfile     : %s" % outfile)
    print("decoded     : %d bytes" % len(raw))
    print("device says : %d bytes  (%s)" % (claimed, "len OK" if ok else "LEN MISMATCH!"))
    print("wire(b64)   : %d bytes  (%.1f%% of raw)" % (len(b64), 100.0 * ratio))
    print("md5         : %s" % md5)
    if not ok:
        sys.exit(3)

if __name__ == "__main__":
    main()

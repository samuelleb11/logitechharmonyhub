#!/usr/bin/env python3
"""netdeploy.py — push a local file to the Harmony hub over the irapi devshell (TCP),
much faster than the UART uploader. Streams base64 in chunks via `printf %s '..' >>`,
decodes on-device with /root/b64d.lua, verifies md5.

Usage:
  tools/netdeploy.py <host> <local_file> <remote_path> [--port 2222] [--token harmonydev]

The devshell protocol: send the token as the first line, then one shell command per
line; each command's stdout (stderr merged) streams back, followed by a "# " prompt.
"""
import socket, sys, base64, hashlib, time

def recv_until_prompt(sock, timeout=15):
    sock.settimeout(timeout)
    buf = b""
    while True:
        try:
            b = sock.recv(4096)
        except socket.timeout:
            return buf, False
        if not b:
            return buf, False
        buf += b
        if buf.endswith(b"# "):
            return buf, True

def main():
    if len(sys.argv) < 4:
        print(__doc__); sys.exit(2)
    host, local, remote = sys.argv[1], sys.argv[2], sys.argv[3]
    port = 2222; token = "harmonydev"
    a = sys.argv[4:]
    for i, v in enumerate(a):
        if v == "--port": port = int(a[i+1])
        if v == "--token": token = a[i+1]

    data = open(local, "rb").read()
    md5 = hashlib.md5(data).hexdigest()
    b64 = base64.b64encode(data).decode("ascii")
    tmp_b64 = remote + ".b64"
    print(f"[netdeploy] {local} -> {host}:{remote}  ({len(data)} bytes, md5 {md5})")

    s = socket.create_connection((host, port), timeout=10)
    s.sendall((token + "\n").encode())
    recv_until_prompt(s)  # greeting + first prompt

    def run(cmd, quiet=False):
        s.sendall((cmd + "\n").encode())
        out, ok = recv_until_prompt(s)
        if not quiet:
            txt = out[:-2].decode("latin-1").strip()
            if txt: print("   " + txt.replace("\n", "\n   "))
        return out

    run(f": > {tmp_b64}", quiet=True)
    CHUNK = 8192
    n = (len(b64) + CHUNK - 1) // CHUNK
    t0 = time.time()
    for i in range(n):
        piece = b64[i*CHUNK:(i+1)*CHUNK]
        run(f"printf %s '{piece}' >> {tmp_b64}", quiet=True)
        if (i+1) % 16 == 0 or i+1 == n:
            sys.stdout.write(f"\r   uploaded {i+1}/{n} chunks"); sys.stdout.flush()
    print(f"   ({time.time()-t0:.1f}s)")

    # decode + chmod + md5, wrapped in a unique sentinel so the result is robust to any
    # prompt-framing drift in the streamed devshell session.
    out = run(f"lua /root/b64d.lua {tmp_b64} {remote} 2>&1; chmod +x {remote}; "
              f"echo SENTINEL_$(md5sum {remote} | cut -d' ' -f1)_SENTINEL")
    run(f"rm -f {tmp_b64}", quiet=True)
    s.sendall(b"exit\n"); s.close()

    import re
    m = re.search(rb"SENTINEL_([0-9a-f]{32})_SENTINEL", out)
    dev_md5 = m.group(1).decode() if m else "?"
    if dev_md5 == md5:
        print(f"[netdeploy] OK — md5 verified ({md5})")
        sys.exit(0)
    else:
        print(f"[netdeploy] MD5 MISMATCH! host={md5} device={dev_md5}")
        sys.exit(1)

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""ota.py — update the Harmony IR appliance over HTTP (no devshell/base64).

  tools/ota.py <host> firmware <binary>   [token]   # flash + reboot into new firmware
  tools/ota.py <host> db       <irdb.txt> [token]    # replace the code DB (hot reload)

Default token = harmonydev. This is the fast replacement for netdeploy.py.
"""
import sys
import urllib.request


def post(url: str, data: bytes) -> tuple[int, str]:
    req = urllib.request.Request(
        url, data=data, method="POST", headers={"Content-Type": "application/octet-stream"}
    )
    with urllib.request.urlopen(req, timeout=90) as r:
        return r.status, r.read().decode(errors="replace")


def main() -> None:
    if len(sys.argv) < 4:
        print(__doc__)
        sys.exit(2)
    host, kind, path = sys.argv[1], sys.argv[2], sys.argv[3]
    token = sys.argv[4] if len(sys.argv) > 4 else "harmonydev"
    data = open(path, "rb").read()
    endpoint = "/api/ota" if kind == "firmware" else "/api/ota/db"
    url = f"http://{host}{endpoint}?token={token}"
    print(f"POST {url}  ({len(data)} bytes from {path})")
    try:
        status, body = post(url, data)
        print(status, body)
    except Exception as e:  # firmware OTA reboots -> the connection can drop after replying
        print(f"note: {e}  (firmware OTA reboots the device; a dropped connection here is normal)")


if __name__ == "__main__":
    main()

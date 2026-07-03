#!/usr/bin/env python3
"""build_irdb.py — curate a broad Flipper-IRDB subset into a COMPACT on-device code DB.

Codes are stored as protocol PARAMS (the device expands them at fire-time via src/enc.rs),
which is ~40 bytes/code vs ~7KB raw — so hundreds/thousands of devices fit. AC/raw captures
stay raw (no lossless param form). Unsupported protocols are skipped.

Output: service/irapi/codes/irdb.txt  (tab-separated)
  D\t<id>\t<type>\t<brand>\t<model>
  F\t<name>\tP\t<protocol>\t<addr_hex>\t<cmd_hex>      (encoded on-device)
  F\t<name>\tR\t<carrier>\t<duty%>\t<us,us,...>         (raw capture)

Usage: tools/build_irdb.py /tmp/flipperdb/Flipper-IRDB-main service/irapi/codes/irdb.txt
"""
import sys, os, re, glob

# Protocols src/enc.rs can expand:
SUPPORTED = {"NEC", "NECext", "NEC42", "NEC42ext", "Samsung32",
             "SIRC", "SIRC15", "SIRC20", "RC5", "RC5X"}

# Categories to include, and how many model files per brand (breadth over depth: 1/brand).
CATEGORIES = [
    ("TVs", 1), ("ACs", 1), ("SoundBars", 1), ("Audio_and_Video_Receivers", 1),
    ("Streaming_Devices", 1), ("Blu-Ray", 1), ("Cable_Boxes", 1), ("DVD_Players", 1),
    ("Projectors", 1), ("Fans", 1), ("Heaters", 1), ("Air_Purifiers", 1),
    ("CD_Players", 1), ("Consoles", 1), ("Speakers", 1), ("Monitors", 1),
]
MAX_DEVICES = 500
MAX_FN = 50

def slug(s):
    return re.sub(r"[^a-z0-9]+", "_", s.lower()).strip("_")

def hexc(s):  # "EE 87 00 00" -> "ee870000"
    return "".join(re.findall(r"[0-9a-fA-F]{2}", s)).lower() or "00000000"

def records(text):
    for block in text.split("#"):
        rec = {}
        for line in block.splitlines():
            m = re.match(r"([\w ]+?):\s*(.*)", line.strip())
            if m:
                rec[m.group(1).strip()] = m.group(2).strip()
        if "name" in rec:
            yield rec

def emit_device(out, did, cat, brand, model, text):
    fns = []
    seen = set()
    for rec in records(text):
        name = rec["name"].replace("\t", " ").strip()
        if not name or name in seen or len(fns) >= MAX_FN:
            continue
        t = rec.get("type")
        if t == "parsed":
            proto = rec.get("protocol", "")
            if proto not in SUPPORTED:
                continue
            addr = hexc(rec.get("address", ""))
            cmd = hexc(rec.get("command", ""))
            fns.append(f"F\t{name}\tP\t{proto}\t{addr}\t{cmd}")
        elif t == "raw":
            us = [x for x in rec.get("data", "").split() if x.isdigit()]
            if not (3 <= len(us) <= 1024):
                continue
            freq = rec.get("frequency", "38000")
            duty = int(round(float(rec.get("duty_cycle", "0.33")) * 100))
            fns.append(f"F\t{name}\tR\t{freq}\t{duty}\t" + ",".join(us))
        else:
            continue
        seen.add(name)
    if not fns:
        return False
    out.append(f"D\t{did}\t{cat}\t{brand}\t{model}")
    out.extend(fns)
    return True

def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "/tmp/flipperdb/Flipper-IRDB-main"
    outp = sys.argv[2] if len(sys.argv) > 2 else "service/irapi/codes/irdb.txt"
    out = []
    n_dev = n_fn = 0
    ids = set()
    for cat, per in CATEGORIES:
        cdir = os.path.join(root, cat)
        if not os.path.isdir(cdir):
            continue
        for brand in sorted(os.listdir(cdir)):
            bdir = os.path.join(cdir, brand)
            if not os.path.isdir(bdir):
                continue
            for fp in sorted(glob.glob(os.path.join(bdir, "*.ir")))[:per]:
                if n_dev >= MAX_DEVICES:
                    break
                model = os.path.splitext(os.path.basename(fp))[0]
                did = slug(f"{brand}_{model}")[:56]
                if did in ids:
                    continue
                text = open(fp, errors="ignore").read()
                before = len(out)
                if emit_device(out, did, cat, brand, model, text):
                    ids.add(did)
                    n_dev += 1
                    n_fn += len(out) - before - 1
    open(outp, "w").write("\n".join(out) + "\n")
    sz = os.path.getsize(outp)
    print(f"wrote {outp}: {n_dev} devices, {n_fn} functions, {sz} bytes ({sz//1024}KB)")

if __name__ == "__main__":
    main()

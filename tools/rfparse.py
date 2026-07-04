#!/usr/bin/env python3
"""rfparse.py — decode Harmony 2.4 GHz RF traffic from hub logs, ON THE MAC.

The hub has only ~30 MB RAM, so parsing capture files on-device OOMs it. Pull the
files to the Mac (e.g. via the devshell) and run this instead. It auto-detects and
decodes three sources of cc2544 RF traffic:

  1. Hub crash dumps  /cache/crashlog-*.json   (libcrashlog.so JSON; we mine the
     captured `syslog` ring buffer for `rf_msg_dump: RF msg ...` hex frames + the
     crash cause). This is what stock `hal` logged while talking to the radio.
  2. rfshim raw log   /cache/rf_hal.log         (our LD_PRELOAD interposer:
     `O|W|R <fd> <n>: <hex...>` — the actual bytes hal write()s/read()s on
     /dev/rfspi; the WRITE side shows the init/reporting/pairing commands).
  3. `irapi rf sniff`  output                    (`[ N B] KIND hex...` describe lines).

Decode tables mirror service/irapi/src/rf.rs (BUTTONS / opcode table) so names stay
consistent across the Rust handler and this parser.

Usage:
  rfparse.py <file|dir> [<file> ...]     # auto-detect + decode each
  rfparse.py --raw <file>                # also dump every frame, not just uniques
"""
import sys, os, json, glob, re

# --- cc2544 opcode table (hal rf_msg_dump @0x461d84), mirrors rf.rs ------------------
OPCODES = {
    0x10: "STATUS/beacon",   # periodic telemetry the radio pushes; hal ignores pre-pairing
    0x11: "LEDS", 0x12: "REPORTING", 0x13: "IRCAMERA_CLOCK", 0x14: "SPEAKER_ENABLE",
    0x15: "STATUS", 0x16: "WRITE_MEM", 0x17: "READ_MEM", 0x18: "SPEAKER_DATA",
    0x1a: "IRCAMERA_ENABLE", 0x20: "STATUS", 0x22: "ACK",
}

# --- Harmony remote HID button command_ids (LE u32 at nRF payload offset 2) ----------
# Mirrors BUTTONS in service/irapi/src/rf.rs.
BUTTONS = {
    0x005800C1: "ok", 0x005200C1: "up", 0x005100C1: "down", 0x005000C1: "left",
    0x004F00C1: "right", 0x006500C1: "menu", 0x005600C1: "prev", 0x002800C1: "enter",
    0x001E00C1: "1", 0x001F00C1: "2", 0x002000C1: "3", 0x002100C1: "4", 0x002200C1: "5",
    0x002300C1: "6", 0x002400C1: "7", 0x002500C1: "8", 0x002600C1: "9", 0x002700C1: "0",
    0x0000E9C3: "vol_up", 0x0000EAC3: "vol_down", 0x00009CC3: "ch_up", 0x00009DC3: "ch_down",
    0x0000E2C3: "mute", 0x000224C3: "back", 0x000094C3: "exit", 0x00009AC3: "dvr",
    0x00008DC3: "guide", 0x0001FFC3: "info", 0x0001F7C3: "red", 0x0001F6C3: "green",
    0x0001F5C3: "yellow", 0x0001F4C3: "blue", 0x0000B4C3: "rewind", 0x0000B3C3: "forward",
    0x0000B0C3: "play", 0x0000B1C3: "pause", 0x0000B7C3: "stop", 0x0000B2C3: "record",
    0x0001E8C3: "music", 0x0001EDC3: "tv", 0x0001E9C3: "movie", 0x0001ECC3: "off",
}


def decode_command_id(b):
    """HID command_id from a 0x20 button-report frame (confirmed live; mirrors rf.rs):
    `20 01 <page> <b3> <b4> …`; page 0x01=keyboard (usage@b4, page 0xC1), 0x03=consumer (usage@b3,
    page 0xC3). usage 0 = release; page 0x41/0x42 = link/idle status."""
    if len(b) < 5 or b[0] != 0x20 or b[1] != 0x01:
        return None
    if b[2] == 0x01:
        usage = (b[3] << 8) | b[4]
        return None if usage == 0 else (usage << 16) | 0xC1
    if b[2] == 0x03:
        usage = (b[4] << 8) | b[3]
        return None if usage == 0 else (usage << 8) | 0xC3
    return None


def decode_button(b):
    code = decode_command_id(b)
    if code is not None and code in BUTTONS:
        return code, BUTTONS[code]
    return None


def describe(b):
    """One-line interpretation of an RF frame (list of ints)."""
    if not b:
        return "empty"
    bt = decode_button(b)
    if bt:
        return "BUTTON %-9s (id 0x%08x)" % (bt[1], bt[0])
    cid = decode_command_id(b)
    if cid is not None:
        return "report UNMAPPED (id 0x%08x)" % cid
    op = b[0]
    if op == 0x10 and len(b) >= 3:
        state = b[2]
        return "STATUS/beacon state=0x%02x %s" % (state, "remote-present" if state & 1 else "idle")
    if op == 0x20:
        return "STATUS/container"
    return OPCODES.get(op, "type=0x%02x" % op)


def hexstr(b):
    return " ".join("%02x" % x for x in b)


def parse_hex_bytes(s):
    toks = re.findall(r"[0-9a-fA-F]{2}", s)
    return [int(t, 16) for t in toks]


# --- source parsers: each yields (tag, bytes) frames -------------------------------
RE_RFDUMP = re.compile(r"rf_msg_dump:\s*RF msg (\w+)\s*#?(\d+)?\s*([0-9a-f ]+)")
RE_RFSHIM = re.compile(r"^([OWR])\s+(-?\d+)\s+(\d+):\s*([0-9a-f ]*)$")
RE_SNIFF = re.compile(r"\[\s*\d+\s*B\]\s+\S.*?((?:[0-9a-f]{2}\s+)+[0-9a-f]{2})\s*$")

NOTABLE_ERR = re.compile(
    r"(binding: Address already in use|I2S_DSIZE|I2S speaker busy|"
    r"segfault|SIGSEGV|SIGABRT|Devices Not Paired|Paired with HUB|"
    r"RF firmware version|PairingStart|PairingStop|pairing)", re.I)


def frames_from_crashlog(path):
    """Yield ('rfdump', kind, bytes) and print the crash summary for a crashlog JSON."""
    txt = open(path, "r", errors="replace").read()
    if not txt.strip():
        print("  (empty file)")
        return
    try:
        j = json.loads(txt)
    except Exception as e:
        print("  ! not valid JSON (%s) — falling back to line scan" % e)
        j = None
    frames = []
    if j is not None:
        cmd = " ".join(j.get("cmdline", []))
        print("  cmdline : %s" % cmd)
        print("  exit    : %s   pid: %s   uptime: %s   fw: %s" %
              (j.get("exit"), j.get("pid"), j.get("system_uptime"), j.get("system_version")))
        maps = j.get("maps", [])
        if any("rfshim" in m for m in maps):
            print("  maps    : /root/rfshim.so IS preloaded (interposer active)")
        syslog = j.get("syslog", [])
        notable = {}
        for line in syslog:
            m = RE_RFDUMP.search(line)
            if m:
                frames.append(("rfdump:" + m.group(1), parse_hex_bytes(m.group(3))))
            for mm in NOTABLE_ERR.finditer(line):
                key = mm.group(1)
                notable[key] = notable.get(key, 0) + 1
        if notable:
            print("  notable :")
            for k, c in sorted(notable.items(), key=lambda x: -x[1]):
                print("      %4dx  %s" % (c, k))
    else:
        for line in txt.splitlines():
            m = RE_RFDUMP.search(line)
            if m:
                frames.append(("rfdump:" + m.group(1), parse_hex_bytes(m.group(3))))
    return frames


def frames_from_plain(path):
    """rfshim raw log or rf-sniff output."""
    frames = []
    for line in open(path, "r", errors="replace"):
        line = line.rstrip("\n")
        m = RE_RFSHIM.match(line.strip())
        if m:
            tag = {"O": "open", "W": "write", "R": "read"}[m.group(1)]
            frames.append((tag, parse_hex_bytes(m.group(4))))
            continue
        m = RE_SNIFF.search(line)
        if m:
            frames.append(("sniff", parse_hex_bytes(m.group(1))))
    return frames


def detect_and_parse(path):
    head = open(path, "r", errors="replace").read(400)
    if head.lstrip().startswith("{") or "crashlog" in os.path.basename(path):
        return frames_from_crashlog(path) or []
    return frames_from_plain(path)


def report(path, show_raw):
    print("=" * 78)
    print("FILE: %s" % path)
    frames = detect_and_parse(path)
    if not frames:
        print("  (no RF frames found)")
        return {}
    # aggregate
    uniq = {}          # hexstr -> [count, tag, bytes]
    buttons = []
    for tag, b in frames:
        h = hexstr(b)
        if h not in uniq:
            uniq[h] = [0, tag, b]
        uniq[h][0] += 1
        bt = decode_button(b)
        if bt:
            buttons.append((tag, bt[1], bt[0], b))
    print("  RF frames: %d total, %d unique" % (len(frames), len(uniq)))
    print("  %-6s %-5s %-40s %s" % ("count", "dir", "bytes", "decode"))
    for h, (c, tag, b) in sorted(uniq.items(), key=lambda x: -x[1][0]):
        print("  %-6d %-5s %-40s %s" % (c, tag.split(":")[-1][:5], h, describe(b)))
    if buttons:
        print("\n  >>> BUTTON PRESSES DECODED (%d):" % len(buttons))
        seen = []
        for tag, name, code, b in buttons:
            if name not in [s[0] for s in seen]:
                seen.append((name, code))
        for name, code in seen:
            print("      %-10s id=0x%08x" % (name, code))
    else:
        print("\n  >>> no button presses in this capture "
              "(only status/telemetry — remote was not paired/pressed, or hal died early)")
    if show_raw:
        print("\n  --- all frames ---")
        for tag, b in frames:
            print("      %-6s %-40s %s" % (tag.split(":")[-1], hexstr(b), describe(b)))
    return uniq


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    show_raw = "--raw" in sys.argv[1:]
    if not args:
        print(__doc__)
        sys.exit(2)
    paths = []
    for a in args:
        if os.path.isdir(a):
            paths += sorted(glob.glob(os.path.join(a, "*")))
        else:
            paths += sorted(glob.glob(a)) or [a]
    for p in paths:
        if os.path.isfile(p):
            report(p, show_raw)
    print("=" * 78)


if __name__ == "__main__":
    main()

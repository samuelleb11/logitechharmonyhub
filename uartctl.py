#!/usr/bin/env python3
"""
uartctl.py - persistent UART console logger + control CLI.

One detached daemon owns the serial port. It:
  * continuously captures EVERY byte from the port into a tmp log file
    (raw fidelity, flushed after every read so nothing is lost even if I
    crash or multi-task), and
  * accepts commands through a FIFO so we can write to the console without
    ever interrupting the capture.

The daemon auto-reconnects if the port drops (e.g. when the target device is
power-cycled) so boot logs are captured across reboots.

Subcommands:
  start [--port P] [--baud B] [--term T]   start the daemon (idempotent)
  stop                                     stop the daemon
  restart                                  stop + start
  status                                   show daemon/port/log status
  send <text...>                           send a line + terminator
  enter                                    send just the terminator (press Enter)
  sendraw <python-escaped>                 send exact bytes (e.g. '\\x03' for Ctrl-C)
  key <name>                               send a named key: ctrl-c ctrl-d esc tab space enter
  spam <python-escaped> <secs>             flood a key for N secs (interrupt autoboot)
  mark <text...>                           write a marker line into the log only
  wait <regex> [timeout=30]                block until regex appears in new log output
  dump [n]                                 print last n bytes of log (default whole/64k)
  log                                      print the active log file path
  tail [n]                                 alias for dump
"""
import os, sys, json, time, signal, base64, re, fcntl, errno

STATE = "/tmp/uartctl.state.json"
DAEMON_ERR = "/tmp/uartctl.daemon.err"
DEFAULT_PORT = "/dev/cu.usbserial-A50285BI"
DEFAULT_BAUD = 115200
DEFAULT_TERM = "\r"   # CR; embedded consoles usually map CR->NL


# ----------------------------- state helpers -----------------------------
def load_state():
    try:
        with open(STATE) as f:
            return json.load(f)
    except Exception:
        return None

def save_state(d):
    tmp = STATE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(d, f)
    os.replace(tmp, STATE)

def alive(pid):
    if not pid:
        return False
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


# ----------------------------- the daemon -------------------------------
def run_daemon(port, baud, term, fifo, logpath):
    import serial  # pyserial
    import threading

    # line-buffered append, binary, never truncate
    logf = open(logpath, "ab", buffering=0)

    def logwrite(b):
        try:
            logf.write(b)
        except Exception:
            pass

    def stamp(msg):
        logwrite(("\n[uartctl %s] %s\n" % (time.strftime("%H:%M:%S"), msg)).encode())

    ser_box = {"ser": None}
    ser_lock = threading.Lock()
    running = {"go": True}

    def open_port():
        while running["go"]:
            try:
                s = serial.Serial(port, baud, timeout=0.15,
                                  bytesize=8, parity="N", stopbits=1,
                                  rtscts=False, dsrdtr=False, xonxoff=False)
                stamp("port OPEN %s @ %d" % (port, baud))
                return s
            except Exception as e:
                stamp("port open failed (%s); retrying" % e)
                time.sleep(0.5)
        return None

    ser_box["ser"] = open_port()

    def reader():
        while running["go"]:
            s = ser_box["ser"]
            if s is None:
                ser_box["ser"] = open_port()
                continue
            try:
                n = s.in_waiting
                data = s.read(n if n else 1)
                if data:
                    logwrite(data)
            except Exception as e:
                stamp("read error (%s); reconnecting" % e)
                try:
                    s.close()
                except Exception:
                    pass
                ser_box["ser"] = None
                time.sleep(0.3)

    def send_bytes(b):
        with ser_lock:
            s = ser_box["ser"]
            if s is None:
                stamp("send dropped (port not open): %r" % b)
                return
            try:
                s.write(b)
                s.flush()
            except Exception as e:
                stamp("write error (%s)" % e)

    t = threading.Thread(target=reader, daemon=True)
    t.start()

    def handle_sig(signum, frame):
        running["go"] = False
    signal.signal(signal.SIGTERM, handle_sig)
    signal.signal(signal.SIGINT, handle_sig)

    term_b = term.encode().decode("unicode_escape").encode("latin-1")

    # FIFO command loop. Each line: "<MODE> <base64>". MODE in TXT/RAW/MARK.
    while running["go"]:
        try:
            with open(fifo, "rb") as f:
                for line in f:
                    line = line.rstrip(b"\n")
                    if not line:
                        continue
                    try:
                        mode, _, b64 = line.partition(b" ")
                        payload = base64.b64decode(b64) if b64 else b""
                        mode = mode.decode()
                    except Exception:
                        continue
                    if mode == "TXT":
                        send_bytes(payload + term_b)
                    elif mode == "RAW":
                        send_bytes(payload)
                    elif mode == "MARK":
                        stamp(payload.decode("latin-1", "replace"))
                    elif mode == "QUIT":
                        running["go"] = False
                        break
        except FileNotFoundError:
            break
        except Exception:
            time.sleep(0.1)

    try:
        if ser_box["ser"]:
            ser_box["ser"].close()
    except Exception:
        pass
    stamp("daemon exit")
    logf.close()


# ----------------------------- client side ------------------------------
def fifo_put(line):
    st = load_state()
    if not st or not alive(st.get("pid")):
        die("daemon not running (use: uartctl.py start)")
    fifo = st["fifo"]
    # non-blocking open so we don't hang if daemon is mid-reopen
    for _ in range(50):
        try:
            fd = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
            break
        except OSError as e:
            if e.errno in (errno.ENXIO, errno.ENOENT):
                time.sleep(0.1)
                continue
            raise
    else:
        die("could not open command FIFO")
    with os.fdopen(fd, "wb") as f:
        f.write(line + b"\n")

def enc(mode, payload: bytes):
    return mode.encode() + b" " + base64.b64encode(payload)

def die(msg):
    print("uartctl: " + msg, file=sys.stderr)
    sys.exit(1)


def cmd_start(args):
    port = DEFAULT_PORT
    baud = DEFAULT_BAUD
    term = DEFAULT_TERM
    i = 0
    while i < len(args):
        if args[i] == "--port": port = args[i+1]; i += 2
        elif args[i] == "--baud": baud = int(args[i+1]); i += 2
        elif args[i] == "--term": term = args[i+1]; i += 2
        else: i += 1

    st = load_state()
    if st and alive(st.get("pid")):
        print("already running (pid %d)" % st["pid"])
        cmd_status([])
        return

    ts = time.strftime("%Y%m%d-%H%M%S")
    logpath = "/tmp/uart-%s.log" % ts
    fifo = "/tmp/uartctl.fifo"
    # fresh FIFO
    try: os.unlink(fifo)
    except FileNotFoundError: pass
    os.mkfifo(fifo)
    # touch log
    open(logpath, "ab").close()
    # stable pointer to latest
    try: os.unlink("/tmp/uart-latest.log")
    except FileNotFoundError: pass
    try: os.symlink(logpath, "/tmp/uart-latest.log")
    except Exception: pass

    # spawn detached daemon
    pid = os.fork()
    if pid == 0:
        os.setsid()
        devnull = os.open(os.devnull, os.O_RDONLY)
        err = os.open(DAEMON_ERR, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
        os.dup2(devnull, 0); os.dup2(err, 1); os.dup2(err, 2)
        try:
            run_daemon(port, baud, term, fifo, logpath)
        finally:
            os._exit(0)
    # parent
    save_state({"pid": pid, "port": port, "baud": baud, "term": term,
                "fifo": fifo, "log": logpath, "started": ts})
    time.sleep(0.6)
    if alive(pid):
        print("started daemon pid %d" % pid)
        cmd_status([])
    else:
        die("daemon failed to start; see " + DAEMON_ERR)


def cmd_stop(args):
    st = load_state()
    if not st:
        print("not running"); return
    pid = st.get("pid")
    try:
        fifo_put(enc("QUIT", b""))
    except SystemExit:
        pass
    time.sleep(0.2)
    if alive(pid):
        try: os.kill(pid, signal.SIGTERM)
        except OSError: pass
        time.sleep(0.3)
    if alive(pid):
        try: os.kill(pid, signal.SIGKILL)
        except OSError: pass
    try: os.unlink(st["fifo"])
    except Exception: pass
    try: os.unlink(STATE)
    except Exception: pass
    print("stopped")


def cmd_status(args):
    st = load_state()
    if not st:
        print("status: NOT RUNNING"); return
    up = alive(st.get("pid"))
    sz = 0
    try: sz = os.path.getsize(st["log"])
    except Exception: pass
    print("status     : %s" % ("RUNNING" if up else "DEAD (stale state)"))
    print("pid        : %s" % st.get("pid"))
    print("port       : %s @ %s" % (st.get("port"), st.get("baud")))
    print("terminator : %r" % st.get("term"))
    print("log        : %s (%d bytes)" % (st.get("log"), sz))
    print("fifo       : %s" % st.get("fifo"))


def cmd_send(args):
    fifo_put(enc("TXT", " ".join(args).encode()))

def cmd_enter(args):
    fifo_put(enc("TXT", b""))

def cmd_sendraw(args):
    raw = " ".join(args).encode().decode("unicode_escape").encode("latin-1")
    fifo_put(enc("RAW", raw))

def cmd_key(args):
    keys = {"ctrl-c": b"\x03", "ctrl-d": b"\x04", "ctrl-z": b"\x1a",
            "esc": b"\x1b", "tab": b"\t", "space": b" ",
            "enter": b"\r", "ctrl-x": b"\x18", "ctrl-a": b"\x01"}
    k = args[0].lower()
    if k not in keys: die("unknown key %r; have %s" % (k, ",".join(keys)))
    fifo_put(enc("RAW", keys[k]))

def cmd_spam(args):
    raw = args[0].encode().decode("unicode_escape").encode("latin-1")
    secs = float(args[1]) if len(args) > 1 else 3.0
    end = time.time() + secs
    while time.time() < end:
        fifo_put(enc("RAW", raw))
        time.sleep(0.02)
    print("spammed %r for %ss" % (raw, secs))

def cmd_mark(args):
    fifo_put(enc("MARK", (" ".join(args)).encode()))

def cmd_wait(args):
    st = load_state()
    if not st: die("daemon not running")
    pat = re.compile(args[0].encode())
    timeout = float(args[1]) if len(args) > 1 else 30.0
    logp = st["log"]
    start_size = os.path.getsize(logp)
    buf = b""
    deadline = time.time() + timeout
    with open(logp, "rb") as f:
        f.seek(start_size)
        while time.time() < deadline:
            chunk = f.read()
            if chunk:
                buf += chunk
                m = pat.search(buf)
                if m:
                    # print context around match
                    tail = buf[max(0, m.start()-200):m.end()+200]
                    sys.stdout.write(tail.decode("utf-8", "replace"))
                    print("\n[uartctl] MATCHED %r" % args[0])
                    return
            else:
                time.sleep(0.1)
    print("[uartctl] TIMEOUT after %ss waiting for %r" % (timeout, args[0]))
    sys.exit(2)

def cmd_dump(args):
    st = load_state()
    if not st: die("daemon not running")
    n = int(args[0]) if args else 65536
    logp = st["log"]
    sz = os.path.getsize(logp)
    with open(logp, "rb") as f:
        if n and sz > n:
            f.seek(sz - n)
        data = f.read()
    sys.stdout.write(data.decode("utf-8", "replace"))
    if not data.endswith(b"\n"):
        sys.stdout.write("\n")

def cmd_log(args):
    st = load_state()
    if not st: die("daemon not running")
    print(st["log"])

def cmd_run(args):
    """Send a shell command, wait for a unique sentinel, print only its output.
    Usage: run [--timeout N] <command...>
    """
    st = load_state()
    if not st: die("daemon not running")
    timeout = 20.0
    if args and args[0] == "--timeout":
        timeout = float(args[1]); args = args[2:]
    cmdline = " ".join(args)
    sent = "__UCTLDONE_%d_%d__" % (os.getpid(), int(time.time() * 1000))
    logp = st["log"]
    start = os.path.getsize(logp)
    # rc is captured too
    fifo_put(enc("TXT", (cmdline + " ; echo " + sent + " rc=$?").encode()))
    needle = ("\n" + sent).encode()
    deadline = time.time() + timeout
    buf = b""
    with open(logp, "rb") as f:
        f.seek(start)
        while time.time() < deadline:
            chunk = f.read()
            if chunk:
                buf += chunk
                idx = buf.find(needle)
                if idx != -1:
                    # output is between end of first line (command echo) and the sentinel line
                    body = buf[:idx]
                    nl = body.find(b"\n")
                    out = body[nl+1:] if nl != -1 else body
                    sys.stdout.write(out.decode("utf-8", "replace"))
                    # echo the rc line
                    after = buf[idx+1:]
                    endnl = after.find(b"\n")
                    rcline = after[:endnl if endnl != -1 else len(after)]
                    sys.stdout.write("\n[" + rcline.decode("utf-8", "replace").strip() + "]\n")
                    return
            else:
                time.sleep(0.05)
    sys.stdout.write(buf.decode("utf-8", "replace"))
    print("\n[uartctl] run TIMEOUT after %ss" % timeout)
    sys.exit(2)


def main():
    if len(sys.argv) < 2:
        print(__doc__); return
    cmd = sys.argv[1]
    rest = sys.argv[2:]
    table = {
        "start": cmd_start, "stop": cmd_stop, "status": cmd_status,
        "restart": lambda a: (cmd_stop(a), time.sleep(0.5), cmd_start(a)),
        "send": cmd_send, "enter": cmd_enter, "sendraw": cmd_sendraw,
        "key": cmd_key, "spam": cmd_spam, "mark": cmd_mark,
        "wait": cmd_wait, "dump": cmd_dump, "tail": cmd_dump, "log": cmd_log,
        "run": cmd_run,
        "_daemon": None,
    }
    if cmd == "_daemon":
        # internal: _daemon port baud term fifo logpath
        run_daemon(rest[0], int(rest[1]), rest[2], rest[3], rest[4]); return
    fn = table.get(cmd)
    if not fn:
        die("unknown command %r\n%s" % (cmd, __doc__))
    fn(rest)

if __name__ == "__main__":
    main()

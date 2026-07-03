#!/usr/bin/env bash
#
# install.sh — guided, (almost) one-touch installer for the Harmony IR appliance.
#
#   Select serial port → install Rust deps → build the firmware → back up the hub
#   → deploy our software. Run it with no arguments for the interactive menu, or:
#
#     ./install.sh deps      install the Rust toolchain + Python deps
#     ./install.sh build     cross-build the firmware (big-endian MIPS)
#     ./install.sh deploy    push firmware over the network (OTA)
#     ./install.sh backup    back up the hub flash over the serial console
#     ./install.sh provision first-time install onto a fresh hub over serial
#     ./install.sh all        deps → build → deploy (the usual update loop)
#
# Choices (serial port, hub IP) are remembered in .install.conf (gitignored).
#
# No `set -e`: this is an interactive tool — a failed/aborted step should drop back to the
# menu, not kill the installer. Critical commands guard themselves with `|| die`.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF="$REPO/.install.conf"
BIN="$REPO/service/irapi/target/mips-unknown-linux-musl/release/irapi"
DB="$REPO/service/irapi/codes/irdb.txt"
RUST_VER="1.74.0"
RUST_TARGET="mips-unknown-linux-musl"

# ---- pretty output -------------------------------------------------------
if [ -t 1 ]; then B=$'\033[1m'; DIM=$'\033[2m'; R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; C=$'\033[36m'; Z=$'\033[0m'; else B= DIM= R= G= Y= C= Z=; fi
say()  { printf '%s\n' "$*"; }
head() { printf '\n%s== %s ==%s\n' "$B" "$*" "$Z"; }
ok()   { printf '%s✓%s %s\n' "$G" "$Z" "$*"; }
warn() { printf '%s!%s %s\n' "$Y" "$Z" "$*"; }
die()  { printf '%s✗ %s%s\n' "$R" "$*" "$Z" >&2; exit 1; }
confirm() { local a; printf '%s? %s [y/N] %s' "$C" "$*" "$Z"; read -r a; [ "$a" = y ] || [ "$a" = Y ]; }

# ---- config persistence --------------------------------------------------
TTY=""; HUB_IP=""; OTA_TOKEN="harmonydev"
[ -f "$CONF" ] && . "$CONF"
save_conf() { printf 'TTY=%q\nHUB_IP=%q\nOTA_TOKEN=%q\n' "$TTY" "$HUB_IP" "$OTA_TOKEN" > "$CONF"; }

OS="$(uname -s)"

# ---- serial port selection ----------------------------------------------
list_ports() {
  if [ "$OS" = Darwin ]; then ls /dev/cu.usbserial-* /dev/cu.usbmodem* 2>/dev/null
  else ls /dev/ttyUSB* /dev/ttyACM* 2>/dev/null; fi
}
pick_tty() {
  head "Select the USB–serial adapter"
  local ports; ports="$(list_ports || true)"
  if [ -z "$ports" ]; then
    warn "No serial devices found (looked for cu.usbserial-*/ttyUSB*)."
    warn "Plug in the FTDI 3.3 V adapter wired to J2 (pad 2 RX, 4 TX, 8 GND)."
    printf '%sEnter a device path manually (or leave blank to skip): %s' "$C" "$Z"; read -r TTY
  else
    local i=1 sel; local arr=()
    while IFS= read -r p; do arr+=("$p"); printf '  %s%d)%s %s\n' "$B" "$i" "$Z" "$p"; i=$((i+1)); done <<< "$ports"
    printf '%sChoose 1-%d (default 1): %s' "$C" "$((i-1))" "$Z"; read -r sel; sel="${sel:-1}"
    TTY="${arr[$((sel-1))]:-}"
  fi
  [ -n "$TTY" ] && { ok "Serial port: $TTY"; save_conf; } || warn "No serial port selected."
}
need_tty() { [ -n "$TTY" ] && [ -e "$TTY" ] || pick_tty; [ -n "$TTY" ] || die "A serial port is required for this step."; }

# ---- 1. dependencies -----------------------------------------------------
step_deps() {
  head "Install build dependencies"
  command -v git   >/dev/null || die "git is required."
  command -v curl  >/dev/null || die "curl is required."
  command -v python3 >/dev/null || die "python3 is required."

  if ! command -v rustup >/dev/null; then
    warn "rustup (the Rust toolchain manager) is not installed."
    if confirm "Install rustup now via https://sh.rustup.rs?"; then
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
      # shellcheck disable=SC1090
      . "$HOME/.cargo/env"
    else
      die "rustup is required. On macOS you can also 'brew install rustup' then 'rustup-init'."
    fi
  fi
  ok "rustup present: $(rustup --version 2>/dev/null || echo '?')"

  say "Installing Rust ${RUST_VER} (last tier-2 for ${RUST_TARGET}) + the MIPS target…"
  rustup toolchain install "$RUST_VER" --profile minimal || die "toolchain install failed"
  rustup target add "$RUST_TARGET" --toolchain "$RUST_VER" || die "target install failed"
  ok "Rust ${RUST_VER} + ${RUST_TARGET} ready."

  if python3 -c 'import serial' 2>/dev/null; then
    ok "python3 pyserial present (needed for the serial console)."
  else
    warn "python3 'pyserial' is missing (needed for serial backup/provision)."
    if confirm "Install pyserial for your user now?"; then
      python3 -m pip install --user pyserial 2>/dev/null \
        || python3 -m pip install --user --break-system-packages pyserial \
        || warn "Could not auto-install; run: python3 -m pip install pyserial"
    fi
  fi
}

# ---- 2. build ------------------------------------------------------------
step_build() {
  head "Build the firmware"
  command -v rustup >/dev/null || die "Run './install.sh deps' first."
  bash "$REPO/service/build-mips.sh" irapi
  [ -f "$BIN" ] || die "Build did not produce $BIN"
  local sz; sz=$(wc -c < "$BIN" | tr -d ' ')
  ok "Built $BIN (${sz} bytes)"
}

# ---- 3. deploy over the network (OTA) -----------------------------------
find_hub() {
  [ -n "$HUB_IP" ] && curl -fsS -m 4 "http://$HUB_IP/api/status" >/dev/null 2>&1 && return 0
  printf '%sHub IP address (e.g. 192.168.1.50): %s' "$C" "$Z"; read -r HUB_IP
  curl -fsS -m 5 "http://$HUB_IP/api/status" >/dev/null 2>&1 || return 1
  save_conf; return 0
}
step_deploy() {
  head "Deploy over the network (OTA)"
  [ -f "$BIN" ] || die "No firmware built yet — run './install.sh build'."
  if ! find_hub; then
    warn "No hub answering at that address."
    warn "If this hub has never run our software, use './install.sh provision' (serial) first."
    return 1
  fi
  ok "Hub is up: $(curl -fsS -m 5 "http://$HUB_IP/api/status")"
  say "Pushing firmware…"
  python3 "$REPO/tools/ota.py" "$HUB_IP" firmware "$BIN" "$OTA_TOKEN" || die "firmware push failed"
  if confirm "Also update the on-device code database (~1.6 MB)?"; then
    python3 "$REPO/tools/ota.py" "$HUB_IP" db "$DB" "$OTA_TOKEN" || warn "database push failed"
  fi
  say "Waiting for the hub to come back…"
  local i
  for i in $(seq 1 30); do
    if curl -fsS -m 4 "http://$HUB_IP/api/status" >/dev/null 2>&1; then
      ok "Hub is back: $(curl -fsS -m 4 "http://$HUB_IP/api/status")"; return 0
    fi
  done
  warn "Hub did not answer within the window — it may still be rebooting."
}

# ---- 4. serial backup ----------------------------------------------------
# Advanced / hardware-gated: the hub must be at a root shell on the console
# (see docs/board-access-runbook.md). Streams every MTD partition to backups/.
step_backup() {
  head "Back up the hub flash (serial)"
  need_tty
  warn "This reads all 16 MB over the 115200 console — expect ~25 minutes."
  warn "The hub must be at a ROOT SHELL on the console first."
  say  "See ${DIM}docs/firmware-backup.md${Z} and ${DIM}docs/board-access-runbook.md${Z} for how to get there."
  confirm "Is the hub at a root shell on ${TTY} now?" || { warn "Aborted."; return 1; }
  python3 "$REPO/tools/uartctl.py" start --port "$TTY" >/dev/null 2>&1 || true
  ( cd "$REPO" && exec bash tools/backup_all.sh )
  ok "Backups written to $REPO/backups/ (md5s are per-unit; a mismatch vs the committed"
  say "  reference just means this is a different hub — that's expected)."
}

# ---- 5. first-time provisioning over serial -----------------------------
# Uploads irapi + code DB + rcS.local to a fresh hub via the console and enables
# them at boot. This automates docs/getting-started.md §"first-time install".
step_provision() {
  head "First-time install onto a fresh hub (serial)"
  [ -f "$BIN" ] || die "No firmware built yet — run './install.sh build'."
  need_tty
  say "This is the ADVANCED path for a hub that has never run our software."
  say "Follow ${DIM}docs/getting-started.md${Z} to get the hub to a root shell on ${TTY},"
  say "then this step base64-uploads the binary + database + boot script over the console."
  warn "It relies on 'base64 -d' being available in the hub's BusyBox; if not, the"
  warn "getting-started guide shows the one-time decoder bootstrap."
  confirm "Is the hub at a root shell on ${TTY} and ready?" || { warn "Aborted."; return 1; }

  python3 "$REPO/tools/uartctl.py" start --port "$TTY" >/dev/null 2>&1 || true
  local uctl="python3 $REPO/tools/uartctl.py"

  if ! $uctl run --timeout 8 'if command -v base64 >/dev/null 2>&1; then echo HAVE_B64; else echo NO_B64; fi' 2>/dev/null | grep -q HAVE_B64; then
    die "The hub's shell has no 'base64' applet. Do the one-time decoder bootstrap in docs/getting-started.md, then re-run."
  fi
  ok "Hub shell reachable; base64 present."

  # stage → base64 → upload → decode on device
  local tmp; tmp="$(mktemp -d)"
  base64 < "$BIN" > "$tmp/irapi.b64"
  base64 < "$DB"  > "$tmp/irdb.b64"
  base64 < "$REPO/service/rcS.local" > "$tmp/rcS.b64"

  say "Uploading firmware (this is the slow part over 115200)…"
  python3 "$REPO/tools/upload_file.py" "$tmp/irapi.b64" /tmp/irapi.b64 || { rm -rf "$tmp"; die "firmware upload failed"; }
  $uctl run --timeout 20 'base64 -d /tmp/irapi.b64 > /root/irapi && chmod +x /root/irapi && echo IRAPI_OK'

  say "Uploading code database…"
  python3 "$REPO/tools/upload_file.py" "$tmp/irdb.b64" /tmp/irdb.b64 || { rm -rf "$tmp"; die "database upload failed"; }
  $uctl run --timeout 20 'mkdir -p /cache 2>/dev/null; base64 -d /tmp/irdb.b64 > /cache/irdb.txt && echo DB_OK'

  say "Uploading boot script…"
  python3 "$REPO/tools/upload_file.py" "$tmp/rcS.b64" /tmp/rcS.b64 || { rm -rf "$tmp"; die "boot-script upload failed"; }
  $uctl run --timeout 15 'base64 -d /tmp/rcS.b64 > /etc/init.d/rcS.local && chmod +x /etc/init.d/rcS.local && echo RCS_OK'

  rm -rf "$tmp"
  ok "Uploaded. Verify md5s on the device, then reboot to start the appliance:"
  say "  ${DIM}$uctl run --timeout 5 'md5sum /root/irapi'${Z}"
  if confirm "Reboot the hub now to launch the appliance?"; then
    $uctl send 'reboot'
    say "Rebooting. Once it joins Wi-Fi, find it and run './install.sh deploy' for future updates."
  fi
}

# ---- menu ----------------------------------------------------------------
menu() {
  head "Harmony IR appliance — installer"
  say "Repo: $REPO"
  [ -n "$TTY" ]    && say "Serial: ${G}$TTY${Z}"    || say "Serial: ${DIM}not set${Z}"
  [ -n "$HUB_IP" ] && say "Hub IP: ${G}$HUB_IP${Z}" || say "Hub IP: ${DIM}not set${Z}"
  cat <<EOF

  ${B}1${Z}) Install build dependencies (Rust ${RUST_VER} + MIPS target)
  ${B}2${Z}) Build the firmware
  ${B}3${Z}) Deploy over the network (OTA) — hub already running our software
  ${B}4${Z}) Select / change the serial port
  ${B}5${Z}) Back up the hub flash (serial)            ${DIM}[advanced]${Z}
  ${B}6${Z}) First-time install onto a fresh hub (serial) ${DIM}[advanced]${Z}
  ${B}a${Z}) Update loop: deps → build → deploy
  ${B}q${Z}) Quit
EOF
  printf '%sChoose: %s' "$C" "$Z"; read -r c || { echo; exit 0; }  # EOF/Ctrl-D → quit
  case "$c" in
    1) step_deps ;;
    2) step_build ;;
    3) step_deploy ;;
    4) pick_tty ;;
    5) step_backup ;;
    6) step_provision ;;
    a|A) step_deps && step_build && step_deploy ;;
    q|Q) exit 0 ;;
    *) warn "Unknown choice." ;;
  esac
}

case "${1:-menu}" in
  deps) step_deps ;;
  build) step_build ;;
  deploy) step_deploy ;;
  backup) step_backup ;;
  provision) step_provision ;;
  tty) pick_tty ;;
  all) step_deps && step_build && step_deploy ;;
  menu) while true; do menu; done ;;
  -h|--help|help) awk 'NR==1{next} /^#/{sub(/^# ?/,"");print;next} {exit}' "$0" ;;
  *) die "Unknown command '$1' (try: deps build deploy backup provision all, or no args for the menu)" ;;
esac

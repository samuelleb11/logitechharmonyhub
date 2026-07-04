"""Constants for the Harmony IR Blaster integration."""

DOMAIN = "harmony_ir"

# options: list of DB device ids to expose as button entities
CONF_DEVICES = "devices"
# options: expose a Midea/Danby AC as a HA climate entity
CONF_AC = "ac"

# custom service
SERVICE_SEND_RAW = "send_raw"

ATTR_RAW_US = "raw_us"
ATTR_CARRIER = "carrier"
ATTR_SELECT = "select"

DEFAULT_CARRIER = 38000
DEFAULT_SELECT = 7  # all three IR emitter outputs

# --- Harmony 2.4GHz remote (RF) ---
# HA event fired (and the `event` entity's event_types) on every paired-remote button press. Users
# trigger automations off the `event` entity or the `harmony_ir_button` bus event.
EVENT_BUTTON = "harmony_ir_button"
RF_POLL_SECONDS = 1  # poll GET /api/rf/recent this often for near-instant button triggers
# The Smart Control button set (mirrors the appliance's rf.rs BUTTONS names) — event_types for the
# remote-button event entity, so automations can trigger on a specific button.
REMOTE_BUTTONS = [
    "off", "music", "tv", "movie",
    "rewind", "play", "forward", "record", "pause", "stop",
    "red", "green", "yellow", "blue",
    "dvr", "guide", "info", "exit", "menu",
    "up", "down", "left", "right", "ok",
    "vol_up", "vol_down", "ch_up", "ch_down", "mute", "back",
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "prev", "enter",
]

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

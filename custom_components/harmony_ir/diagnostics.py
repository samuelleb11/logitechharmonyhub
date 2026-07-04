"""Diagnostics for Harmony IR — a one-click support bundle from the entry ⋮ menu."""
from __future__ import annotations

from typing import Any

from homeassistant.components.diagnostics import async_redact_data
from homeassistant.const import CONF_HOST
from homeassistant.core import HomeAssistant

from .coordinator import HarmonyConfigEntry

TO_REDACT = {CONF_HOST, "ip", "ssid"}  # LAN/location identifiers; this device has no credentials


async def async_get_config_entry_diagnostics(
    hass: HomeAssistant, entry: HarmonyConfigEntry
) -> dict[str, Any]:
    """Return redacted config + latest polled status for support."""
    coordinator = entry.runtime_data
    return {
        "entry_data": async_redact_data(entry.data, TO_REDACT),
        "entry_options": async_redact_data(dict(entry.options), TO_REDACT),
        "coordinator_data": async_redact_data(coordinator.data or {}, TO_REDACT),
    }

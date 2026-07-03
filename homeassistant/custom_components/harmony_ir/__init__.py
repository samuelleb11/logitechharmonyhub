"""The Harmony IR Blaster integration."""
from __future__ import annotations

import voluptuous as vol

from homeassistant.const import CONF_HOST, Platform
from homeassistant.core import HomeAssistant, ServiceCall
from homeassistant.exceptions import HomeAssistantError, ServiceValidationError
from homeassistant.helpers import config_validation as cv, entity_registry as er
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.typing import ConfigType

from .api import ApiClient, ApiError
from .const import (
    ATTR_CARRIER,
    ATTR_RAW_US,
    ATTR_SELECT,
    DEFAULT_CARRIER,
    DEFAULT_SELECT,
    DOMAIN,
    SERVICE_SEND_RAW,
)
from .coordinator import HarmonyConfigEntry, HarmonyCoordinator

PLATFORMS: list[Platform] = [
    Platform.REMOTE,
    Platform.BUTTON,
    Platform.CLIMATE,
    Platform.SENSOR,
]

SEND_RAW_SCHEMA = vol.Schema(
    {
        vol.Required("entity_id"): cv.entity_ids,
        vol.Required(ATTR_RAW_US): vol.All(cv.ensure_list, [vol.Coerce(int)]),
        vol.Optional(ATTR_CARRIER, default=DEFAULT_CARRIER): vol.Coerce(int),
        vol.Optional(ATTR_SELECT, default=DEFAULT_SELECT): vol.Coerce(int),
    }
)


async def async_setup(hass: HomeAssistant, config: ConfigType) -> bool:
    """Register the domain-level send_raw action once (raw µs isn't expressible via remote)."""

    async def _send_raw(call: ServiceCall) -> None:
        registry = er.async_get(hass)
        targets: set[str] = set()
        for entity_id in call.data["entity_id"]:
            rentry = registry.async_get(entity_id)
            if rentry is None or rentry.config_entry_id is None:
                raise ServiceValidationError(f"Unknown entity: {entity_id}")
            targets.add(rentry.config_entry_id)
        for entry_id in targets:
            entry = hass.config_entries.async_get_entry(entry_id)
            if entry is None or getattr(entry, "runtime_data", None) is None:
                raise ServiceValidationError("Target Harmony IR entry is not loaded")
            coordinator: HarmonyCoordinator = entry.runtime_data
            try:
                await coordinator.client.send_raw(
                    call.data[ATTR_RAW_US], call.data[ATTR_CARRIER], call.data[ATTR_SELECT]
                )
            except ApiError as err:
                raise HomeAssistantError(f"send_raw failed: {err}") from err

    hass.services.async_register(DOMAIN, SERVICE_SEND_RAW, _send_raw, schema=SEND_RAW_SCHEMA)
    return True


async def async_setup_entry(hass: HomeAssistant, entry: HarmonyConfigEntry) -> bool:
    """Set up Harmony IR from a config entry."""
    client = ApiClient(async_get_clientsession(hass), entry.data[CONF_HOST])
    coordinator = HarmonyCoordinator(hass, entry, client)
    await coordinator.async_config_entry_first_refresh()  # raises ConfigEntryNotReady if down
    entry.runtime_data = coordinator
    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    entry.async_on_unload(entry.add_update_listener(_async_update_listener))
    return True


async def _async_update_listener(hass: HomeAssistant, entry: HarmonyConfigEntry) -> None:
    """Reload so button/AC entities match the newly-configured options."""
    await hass.config_entries.async_reload(entry.entry_id)


async def async_unload_entry(hass: HomeAssistant, entry: HarmonyConfigEntry) -> bool:
    """Unload a config entry (the send_raw action is domain-global and stays registered)."""
    return await hass.config_entries.async_unload_platforms(entry, PLATFORMS)

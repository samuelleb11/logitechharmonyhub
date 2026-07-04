"""DataUpdateCoordinator + shared hub device-info for Harmony IR.

Polls GET /api/status so every entity can go *unavailable* when the appliance drops,
and exposes the single canonical hub DeviceInfo that all hub-level entities share.
"""
from __future__ import annotations

from datetime import timedelta
import logging
from typing import Any

from homeassistant.config_entries import ConfigEntry
from homeassistant.const import CONF_HOST
from homeassistant.core import HomeAssistant
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.update_coordinator import DataUpdateCoordinator, UpdateFailed

from .api import ApiClient, ApiError
from .const import DOMAIN, RF_POLL_SECONDS

_LOGGER = logging.getLogger(__name__)

type HarmonyData = dict[str, Any]  # coordinator.data == the raw /api/status payload
type HarmonyConfigEntry = ConfigEntry[HarmonyCoordinator]  # entry.runtime_data is the coordinator


class HarmonyCoordinator(DataUpdateCoordinator[HarmonyData]):
    """Poll GET /api/status every 30 s (gentle on the AR9331)."""

    config_entry: HarmonyConfigEntry  # class annotation narrows the type

    def __init__(
        self, hass: HomeAssistant, config_entry: HarmonyConfigEntry, client: ApiClient
    ) -> None:
        super().__init__(
            hass,
            _LOGGER,
            config_entry=config_entry,  # keyword-only — always pass it
            name=DOMAIN,
            update_interval=timedelta(seconds=30),
        )
        self.client = client  # platforms reach the appliance API via coordinator.client
        self.rf: HarmonyRfCoordinator | None = None  # set in async_setup_entry (remote-button feed)

    async def _async_update_data(self) -> HarmonyData:
        try:
            return await self.client.status()
        except ApiError as err:
            raise UpdateFailed(f"Harmony IR status failed: {err}") from err


def hub_device_info(coordinator: HarmonyCoordinator) -> DeviceInfo:
    """THE single hub device. Every hub-level entity must use this verbatim so they group."""
    entry = coordinator.config_entry
    return DeviceInfo(
        identifiers={(DOMAIN, entry.entry_id)},
        name="Harmony IR Blaster",
        manufacturer="Logitech (re-flashed Harmony Hub)",
        model="AR9331 Direct-I2S IR blaster",
        sw_version=(coordinator.data or {}).get("version"),
        configuration_url=f"http://{entry.data[CONF_HOST]}",
    )


class HarmonyRfCoordinator(DataUpdateCoordinator[HarmonyData]):
    """Poll GET /api/rf/recent frequently for near-instant remote-button triggers.

    Best-effort: a failed poll (hub blip, or older firmware without /api/rf/recent) yields the last
    payload instead of failing, so the remote-button event entity never flaps and the rest of the
    integration is unaffected.
    """

    config_entry: HarmonyConfigEntry

    def __init__(
        self, hass: HomeAssistant, config_entry: HarmonyConfigEntry, client: ApiClient
    ) -> None:
        super().__init__(
            hass,
            _LOGGER,
            config_entry=config_entry,
            name=f"{DOMAIN}_rf",
            update_interval=timedelta(seconds=RF_POLL_SECONDS),
        )
        self.client = client

    async def _async_update_data(self) -> HarmonyData:
        try:
            return await self.client.rf_recent()
        except ApiError:
            return self.data or {}  # keep last; don't flap on a transient blip / missing endpoint

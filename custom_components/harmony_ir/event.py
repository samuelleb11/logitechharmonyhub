"""Event entity for the paired Harmony 2.4GHz remote.

Turns every remote button press into a Home Assistant trigger. The appliance's `rf listen` daemon
decodes presses and records the latest to GET /api/rf/recent (with a monotonic `seq`); the fast RF
coordinator polls it, and this entity fires when `seq` advances. Two ways to trigger automations:

  * the `event` entity itself (device/entity trigger; `event_type` = the button name), or
  * the `harmony_ir_button` bus event: {"button": "vol_up", "id": "0x0000e9c3", "action": "none"}.

A button mapped to a local IR code still fires here too, so one button can do both.
"""
from __future__ import annotations

import logging

from homeassistant.components.event import EventDeviceClass, EventEntity
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from .const import EVENT_BUTTON, REMOTE_BUTTONS
from .coordinator import HarmonyConfigEntry, HarmonyRfCoordinator, hub_device_info

_LOGGER = logging.getLogger(__name__)


async def async_setup_entry(
    hass: HomeAssistant,
    entry: HarmonyConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Add the remote-button event entity (only if the appliance exposes the RF endpoints)."""
    coordinator = entry.runtime_data
    rf = getattr(coordinator, "rf", None)
    if rf is None:
        return
    async_add_entities([HarmonyRemoteButtonEvent(coordinator, rf)])


class HarmonyRemoteButtonEvent(CoordinatorEntity[HarmonyRfCoordinator], EventEntity):
    """Fires on each paired-remote button press; event_types are the button names."""

    _attr_has_entity_name = True
    _attr_name = "Remote button"
    _attr_device_class = EventDeviceClass.BUTTON
    _attr_event_types = REMOTE_BUTTONS

    def __init__(self, coordinator, rf: HarmonyRfCoordinator) -> None:
        super().__init__(rf)
        self._attr_unique_id = f"{coordinator.config_entry.entry_id}_remote_button"
        self._attr_device_info = hub_device_info(coordinator)
        self._last_seq: int | None = None

    @callback
    def _handle_coordinator_update(self) -> None:
        data = self.coordinator.data or {}
        seq = data.get("seq")
        button = data.get("button")
        if seq is None or button is None:
            return
        if self._last_seq is None:
            self._last_seq = seq  # first snapshot may be a stale press — arm, don't fire
            return
        if seq == self._last_seq:
            return
        self._last_seq = seq
        if button not in self._attr_event_types:
            return
        attrs = {"id": data.get("id"), "action": data.get("action")}
        self._trigger_event(button, attrs)
        # also fire a plain bus event so `event_type: harmony_ir_button` automations work too
        self.hass.bus.async_fire(EVENT_BUTTON, {"button": button, **attrs})
        self.async_write_ha_state()

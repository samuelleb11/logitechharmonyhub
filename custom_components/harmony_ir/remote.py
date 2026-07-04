"""Remote platform for Harmony IR Blaster — fires DB codes and learns/deletes codes natively.

Usage in HA:
    service: remote.send_command
    target: { entity_id: remote.harmony_ir_blaster }
    data: { device: "tv_samsung_samsung", command: ["Power"], num_repeats: 1 }

    service: remote.learn_command      # press the physical remote when prompted
    data: { entity_id: remote.harmony_ir_blaster, device: "bedroom_tv", command: ["Power"] }

    service: remote.delete_command
    data: { entity_id: remote.harmony_ir_blaster, device: "bedroom_tv", command: ["Power"] }
"""
from __future__ import annotations

import asyncio
from collections.abc import Iterable
import logging
from typing import Any

from homeassistant.components import persistent_notification
from homeassistant.components.remote import (
    ATTR_DELAY_SECS,
    ATTR_DEVICE,
    ATTR_NUM_REPEATS,
    ATTR_TIMEOUT,
    DEFAULT_DELAY_SECS,
    DEFAULT_NUM_REPEATS,
    RemoteEntity,
    RemoteEntityFeature,
)
from homeassistant.const import ATTR_COMMAND
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import HomeAssistantError, ServiceValidationError
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from .api import ApiError
from .const import DEFAULT_CARRIER
from .coordinator import HarmonyConfigEntry, HarmonyCoordinator, hub_device_info

_LOGGER = logging.getLogger(__name__)
PARALLEL_UPDATES = 1  # a single IR emitter — never blast two frames at once
LEARN_NOTIFICATION_ID = "harmony_ir_learn"
DEFAULT_LEARN_TIMEOUT_S = 15


async def async_setup_entry(
    hass: HomeAssistant,
    entry: HarmonyConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Set up the remote entity."""
    async_add_entities([HarmonyIrRemote(entry.runtime_data)])


class HarmonyIrRemote(CoordinatorEntity[HarmonyCoordinator], RemoteEntity):
    """A remote that forwards commands to the appliance's offline IR database."""

    _attr_has_entity_name = True
    _attr_name = None  # main feature of the hub device → inherits the device name
    _attr_supported_features = (
        RemoteEntityFeature.LEARN_COMMAND | RemoteEntityFeature.DELETE_COMMAND
    )

    def __init__(self, coordinator: HarmonyCoordinator) -> None:
        super().__init__(coordinator)
        self._client = coordinator.client
        self._attr_unique_id = f"{coordinator.config_entry.entry_id}_remote"
        self._attr_is_on = True
        self._attr_device_info = hub_device_info(coordinator)

    async def async_turn_on(self, **kwargs: Any) -> None:
        self._attr_is_on = True
        self.async_write_ha_state()

    async def async_turn_off(self, **kwargs: Any) -> None:
        self._attr_is_on = False
        self.async_write_ha_state()

    async def async_send_command(self, command: Iterable[str], **kwargs: Any) -> None:
        device = kwargs.get(ATTR_DEVICE)
        if not device:
            raise ServiceValidationError(
                "'device' is required — a DB device id (browse the hub web UI)"
            )
        repeats = int(kwargs.get(ATTR_NUM_REPEATS, DEFAULT_NUM_REPEATS))
        delay = float(kwargs.get(ATTR_DELAY_SECS, DEFAULT_DELAY_SECS))
        commands = list(command)
        first = True
        for _ in range(max(1, repeats)):
            for function in commands:
                if not first:
                    await asyncio.sleep(delay)  # gap BETWEEN emissions, not before the first
                first = False
                try:
                    await self._client.send(device, function)
                except ApiError as err:
                    raise HomeAssistantError(
                        f"send {device}/{function} failed: {err}"
                    ) from err

    async def async_learn_command(self, **kwargs: Any) -> None:
        device = kwargs.get(ATTR_DEVICE)
        commands = kwargs.get(ATTR_COMMAND) or []
        timeout_s = int(kwargs.get(ATTR_TIMEOUT) or DEFAULT_LEARN_TIMEOUT_S)
        for function in commands:
            persistent_notification.async_create(
                self.hass,
                f"Press the '{function}' button on your remote now (aim it at the hub front).",
                title="Harmony IR: learn command",
                notification_id=LEARN_NOTIFICATION_ID,
            )
            try:
                result = await self._client.learn(timeout_ms=timeout_s * 1000)
                if not result.get("us"):
                    _LOGGER.error("No IR code captured for '%s'", function)
                    continue
                await self._client.learn_save(
                    function=function,
                    carrier=int(result.get("carrier", DEFAULT_CARRIER)),
                    us=result["us"],
                    device=device,  # None ⇒ the firmware allocates a custom device id
                )
                _LOGGER.info("Learned '%s'%s", function, f" for {device}" if device else "")
            except ApiError as err:
                _LOGGER.error("Failed to learn '%s': %s", function, err)
            finally:
                persistent_notification.async_dismiss(
                    self.hass, notification_id=LEARN_NOTIFICATION_ID
                )

    async def async_delete_command(self, **kwargs: Any) -> None:
        device = kwargs.get(ATTR_DEVICE)
        if not device:
            raise ServiceValidationError("'device' is required to delete a command")
        for function in kwargs.get(ATTR_COMMAND) or []:
            try:
                await self._client.forget(device, function)
            except ApiError as err:
                _LOGGER.error("Failed to forget '%s': %s", function, err)

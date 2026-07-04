"""Diagnostic sensors for the Harmony IR appliance (hub health, coordinator-fed)."""
from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta

from homeassistant.components.sensor import (
    SensorDeviceClass,
    SensorEntity,
    SensorEntityDescription,
)
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback
from homeassistant.helpers.typing import StateType
from homeassistant.helpers.update_coordinator import CoordinatorEntity
from homeassistant.util import dt as dt_util

from .coordinator import (
    HarmonyConfigEntry,
    HarmonyCoordinator,
    HarmonyData,
    hub_device_info,
)

PARALLEL_UPDATES = 0  # read-only, coordinator-fed


@dataclass(frozen=True, kw_only=True)
class HarmonySensorDescription(SensorEntityDescription):
    """A diagnostic sensor described by a value function over the /api/status payload."""

    value_fn: Callable[[HarmonyData], StateType | datetime]


def _last_boot(data: HarmonyData) -> datetime | None:
    """Convert the rising 'uptime' seconds counter into a stable 'last boot' timestamp."""
    up = data.get("uptime")
    if up in (None, ""):
        return None
    try:
        seconds = int(up)
    except (TypeError, ValueError):
        return None
    boot = dt_util.utcnow() - timedelta(seconds=seconds)
    return boot.replace(second=0, microsecond=0)  # round to the minute to kill ±1 s jitter


SENSORS: tuple[HarmonySensorDescription, ...] = (
    HarmonySensorDescription(
        key="status",
        translation_key="status",
        entity_category=EntityCategory.DIAGNOSTIC,
        value_fn=lambda d: d.get("mode"),
    ),
    HarmonySensorDescription(
        key="ssid",
        translation_key="ssid",
        entity_category=EntityCategory.DIAGNOSTIC,
        value_fn=lambda d: d.get("ssid") or None,
    ),
    HarmonySensorDescription(
        key="ip_address",
        translation_key="ip_address",
        entity_category=EntityCategory.DIAGNOSTIC,
        entity_registry_enabled_default=False,  # LAN address → off by default
        value_fn=lambda d: d.get("ip") or None,
    ),
    HarmonySensorDescription(
        key="firmware",
        translation_key="firmware",
        entity_category=EntityCategory.DIAGNOSTIC,
        value_fn=lambda d: d.get("version"),
    ),
    HarmonySensorDescription(
        key="ir_state",
        translation_key="ir_state",
        entity_category=EntityCategory.DIAGNOSTIC,
        value_fn=lambda d: d.get("ir"),
    ),
    HarmonySensorDescription(
        key="last_boot",
        translation_key="last_boot",
        entity_category=EntityCategory.DIAGNOSTIC,
        device_class=SensorDeviceClass.TIMESTAMP,
        value_fn=_last_boot,
    ),
)


async def async_setup_entry(
    hass: HomeAssistant,
    entry: HarmonyConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Set up the diagnostic sensors."""
    coordinator = entry.runtime_data
    async_add_entities(HarmonyDiagnosticSensor(coordinator, d) for d in SENSORS)


class HarmonyDiagnosticSensor(CoordinatorEntity[HarmonyCoordinator], SensorEntity):
    """A single diagnostic value from /api/status."""

    _attr_has_entity_name = True
    entity_description: HarmonySensorDescription

    def __init__(
        self, coordinator: HarmonyCoordinator, description: HarmonySensorDescription
    ) -> None:
        super().__init__(coordinator)
        self.entity_description = description
        self._attr_unique_id = f"{coordinator.config_entry.entry_id}_{description.key}"
        self._attr_device_info = hub_device_info(coordinator)

    @property
    def native_value(self) -> StateType | datetime:
        return self.entity_description.value_fn(self.coordinator.data or {})

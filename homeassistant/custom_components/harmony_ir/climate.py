"""Climate entity for a Midea/Danby AC driven through the Harmony IR appliance.

IR is one-way (fire-and-forget) — there is no feedback from the AC — so this entity is
"optimistic": it stores the last commanded state and re-sends the full state on every change,
exactly like the physical remote (every button press transmits the complete frame)."""
from __future__ import annotations

from typing import Any

from homeassistant.components.climate import (
    ClimateEntity,
    ClimateEntityFeature,
    HVACAction,
    HVACMode,
)
from homeassistant.const import ATTR_TEMPERATURE, UnitOfTemperature
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from .api import ApiError
from .const import CONF_AC
from .coordinator import HarmonyConfigEntry, HarmonyCoordinator, hub_device_info

PARALLEL_UPDATES = 1  # single IR emitter

# HA HVAC mode  <->  appliance /api/ac/send "mode" string
_HVAC_TO_MODE = {
    HVACMode.COOL: "cool",
    HVACMode.HEAT: "heat",
    HVACMode.DRY: "dry",
    HVACMode.FAN_ONLY: "fan",
    HVACMode.AUTO: "auto",
}
_HVAC_TO_ACTION = {
    HVACMode.OFF: HVACAction.OFF,
    HVACMode.COOL: HVACAction.COOLING,
    HVACMode.HEAT: HVACAction.HEATING,
    HVACMode.DRY: HVACAction.DRYING,
    HVACMode.FAN_ONLY: HVACAction.FAN,
    HVACMode.AUTO: HVACAction.IDLE,
}
_MIN_TEMP = 17
_MAX_TEMP = 31
_FAN_MODES = ["auto", "low", "medium", "high"]


async def async_setup_entry(
    hass: HomeAssistant,
    entry: HarmonyConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Set up the AC climate entity if enabled in options."""
    if not entry.options.get(CONF_AC):
        return
    async_add_entities([HarmonyClimate(entry.runtime_data)])


class HarmonyClimate(CoordinatorEntity[HarmonyCoordinator], ClimateEntity):
    """An optimistic Midea/Danby AC (temp/mode/fan) over IR."""

    _attr_has_entity_name = True
    _attr_translation_key = "air_conditioner"
    _attr_temperature_unit = UnitOfTemperature.CELSIUS
    _attr_target_temperature_step = 1.0
    _attr_min_temp = _MIN_TEMP
    _attr_max_temp = _MAX_TEMP
    _attr_fan_modes = _FAN_MODES
    _attr_hvac_modes = [HVACMode.OFF, *_HVAC_TO_MODE]
    _attr_supported_features = (
        ClimateEntityFeature.TARGET_TEMPERATURE
        | ClimateEntityFeature.FAN_MODE
        | ClimateEntityFeature.TURN_ON
        | ClimateEntityFeature.TURN_OFF
    )

    def __init__(self, coordinator: HarmonyCoordinator) -> None:
        super().__init__(coordinator)
        self._client = coordinator.client
        self._attr_unique_id = f"{coordinator.config_entry.entry_id}_ac"
        self._attr_device_info = hub_device_info(coordinator)
        # last commanded (optimistic) state — must be initialized (no core defaults)
        self._attr_hvac_mode = HVACMode.OFF
        self._last_active_mode = HVACMode.COOL
        self._attr_fan_mode = "auto"
        self._attr_target_temperature = 22

    @property
    def hvac_action(self) -> HVACAction | None:
        """Coarse action mirror of the commanded mode (no sensor feedback to do better)."""
        return _HVAC_TO_ACTION.get(self._attr_hvac_mode)

    async def _push(self) -> None:
        """Send the full current state to the AC (mirrors the remote's whole-frame behaviour)."""
        power = self._attr_hvac_mode != HVACMode.OFF
        mode = _HVAC_TO_MODE.get(
            self._attr_hvac_mode if power else self._last_active_mode, "cool"
        )
        try:
            await self._client.ac_send(
                power, mode, self._attr_fan_mode or "auto", int(self._attr_target_temperature)
            )
        except ApiError as err:
            raise HomeAssistantError(f"AC send failed: {err}") from err
        self.async_write_ha_state()

    async def async_set_hvac_mode(self, hvac_mode: HVACMode) -> None:
        self._attr_hvac_mode = hvac_mode
        if hvac_mode != HVACMode.OFF:
            self._last_active_mode = hvac_mode
        await self._push()

    async def async_set_fan_mode(self, fan_mode: str) -> None:
        self._attr_fan_mode = fan_mode
        await self._push()

    async def async_set_temperature(self, **kwargs: Any) -> None:
        if (temp := kwargs.get(ATTR_TEMPERATURE)) is not None:
            self._attr_target_temperature = round(temp)
        await self._push()

    async def async_turn_on(self) -> None:
        self._attr_hvac_mode = self._last_active_mode
        await self._push()

    async def async_turn_off(self) -> None:
        self._attr_hvac_mode = HVACMode.OFF
        await self._push()

"""Button platform — one pressable button per function of each UI-configured device."""
from __future__ import annotations

from homeassistant.components.button import ButtonEntity
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from .api import ApiError
from .const import CONF_DEVICES, DOMAIN
from .coordinator import HarmonyConfigEntry, HarmonyCoordinator

PARALLEL_UPDATES = 1  # single IR emitter


async def async_setup_entry(
    hass: HomeAssistant,
    entry: HarmonyConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Create button entities for each device the user added in the options UI."""
    coordinator = entry.runtime_data
    device_ids: list[str] = list(entry.options.get(CONF_DEVICES, []))
    if not device_ids:
        return

    # Map id -> (brand, model) for nice names/grouping.
    try:
        meta = {d["id"]: d for d in await coordinator.client.devices()}
    except ApiError:
        meta = {}

    entities: list[HarmonyIrButton] = []
    for dev_id in device_ids:
        try:
            functions = await coordinator.client.functions(dev_id)
        except ApiError:
            continue
        info = meta.get(dev_id, {})
        label = info.get("model") or dev_id
        brand = info.get("brand", "")
        for fn in functions:
            entities.append(HarmonyIrButton(coordinator, dev_id, label, brand, fn))
    async_add_entities(entities)


class HarmonyIrButton(CoordinatorEntity[HarmonyCoordinator], ButtonEntity):
    """A single IR function as a Home Assistant button."""

    _attr_has_entity_name = True

    def __init__(
        self,
        coordinator: HarmonyCoordinator,
        device_id: str,
        label: str,
        brand: str,
        function: str,
    ) -> None:
        super().__init__(coordinator)
        self._client = coordinator.client
        entry = coordinator.config_entry
        self._device_id = device_id
        self._function = function
        self._attr_name = function
        self._attr_unique_id = f"{entry.entry_id}_{device_id}_{function}"
        self._attr_device_info = DeviceInfo(
            identifiers={(DOMAIN, f"{entry.entry_id}_{device_id}")},
            name=f"{brand} {label}".strip(),
            manufacturer=brand or "Harmony IR",
            model=label,
            via_device=(DOMAIN, entry.entry_id),
        )

    async def async_press(self) -> None:
        try:
            await self._client.send(self._device_id, self._function)
        except ApiError as err:
            raise HomeAssistantError(
                f"harmony_ir: {self._device_id}/{self._function}: {err}"
            ) from err

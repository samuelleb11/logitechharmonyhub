"""Config + options flow for Harmony IR Blaster (fully UI-driven)."""
from __future__ import annotations

from typing import Any

import voluptuous as vol

from homeassistant.config_entries import (
    ConfigEntry,
    ConfigFlow,
    ConfigFlowResult,
    OptionsFlow,
)
from homeassistant.const import CONF_HOST
from homeassistant.core import callback
from homeassistant.helpers import config_validation as cv
from homeassistant.helpers.aiohttp_client import async_get_clientsession

from .api import ApiClient, ApiError
from .const import CONF_AC, CONF_DEVICES, DOMAIN


class HarmonyIrConfigFlow(ConfigFlow, domain=DOMAIN):
    """Handle the initial setup (host)."""

    VERSION = 1

    async def async_step_user(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        errors: dict[str, str] = {}
        if user_input is not None:
            host = user_input[CONF_HOST].strip()
            client = ApiClient(async_get_clientsession(self.hass), host)
            try:
                status = await client.status()
            except ApiError:
                errors["base"] = "cannot_connect"
            else:
                await self.async_set_unique_id(host)
                self._abort_if_unique_id_configured()
                return self.async_create_entry(
                    title=f"Harmony IR ({status.get('ip', host)})",
                    data={CONF_HOST: host},
                )
        return self.async_show_form(
            step_id="user",
            data_schema=vol.Schema({vol.Required(CONF_HOST): str}),
            errors=errors,
        )

    async def async_step_reconfigure(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        """Change the appliance host without removing the integration."""
        entry = self._get_reconfigure_entry()
        errors: dict[str, str] = {}
        if user_input is not None:
            host = user_input[CONF_HOST].strip()
            client = ApiClient(async_get_clientsession(self.hass), host)
            try:
                await client.status()  # test-before-configure
            except ApiError:
                errors["base"] = "cannot_connect"
            else:
                return self.async_update_reload_and_abort(
                    entry, data_updates={CONF_HOST: host}
                )
        return self.async_show_form(
            step_id="reconfigure",
            data_schema=self.add_suggested_values_to_schema(
                vol.Schema({vol.Required(CONF_HOST): str}),
                {CONF_HOST: entry.data[CONF_HOST]},
            ),
            errors=errors,
        )

    @staticmethod
    @callback
    def async_get_options_flow(config_entry: ConfigEntry) -> OptionsFlow:
        return HarmonyIrOptionsFlow(config_entry)


class HarmonyIrOptionsFlow(OptionsFlow):
    """UI options: browse the on-device library and add/remove devices as button entities."""

    def __init__(self, entry: ConfigEntry) -> None:
        self._entry = entry
        self._client: ApiClient | None = None
        self._pick: dict[str, str] = {}

    def _api(self) -> ApiClient:
        if self._client is None:
            self._client = ApiClient(
                async_get_clientsession(self.hass), self._entry.data[CONF_HOST]
            )
        return self._client

    def _configured(self) -> list[str]:
        return list(self._entry.options.get(CONF_DEVICES, []))

    async def async_step_init(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        return self.async_show_menu(
            step_id="init", menu_options=["add_type", "remove", "ac", "done"]
        )

    async def async_step_ac(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        """Enable/disable the Midea/Danby AC climate entity."""
        if user_input is not None:
            return self.async_create_entry(
                title="", data={**self._entry.options, CONF_AC: user_input[CONF_AC]}
            )
        return self.async_show_form(
            step_id="ac",
            data_schema=vol.Schema(
                {
                    vol.Required(
                        CONF_AC, default=bool(self._entry.options.get(CONF_AC))
                    ): bool
                }
            ),
        )

    async def async_step_add_type(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        if user_input is not None:
            self._pick["type"] = user_input["type"]
            return await self.async_step_add_brand()
        try:
            types = await self._api().types()
        except ApiError:
            return self.async_abort(reason="cannot_connect")
        return self.async_show_form(
            step_id="add_type", data_schema=vol.Schema({vol.Required("type"): vol.In(types)})
        )

    async def async_step_add_brand(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        if user_input is not None:
            self._pick["brand"] = user_input["brand"]
            return await self.async_step_add_model()
        brands = await self._api().brands(self._pick["type"])
        return self.async_show_form(
            step_id="add_brand", data_schema=vol.Schema({vol.Required("brand"): vol.In(brands)})
        )

    async def async_step_add_model(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        if user_input is not None:
            devices = self._configured()
            if user_input["model"] not in devices:
                devices.append(user_input["model"])
            return self.async_create_entry(
                title="", data={**self._entry.options, CONF_DEVICES: devices}
            )
        devs = await self._api().devices(self._pick["type"], self._pick["brand"])
        choices = {d["id"]: d.get("model", d["id"]) for d in devs}
        return self.async_show_form(
            step_id="add_model", data_schema=vol.Schema({vol.Required("model"): vol.In(choices)})
        )

    async def async_step_remove(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        configured = self._configured()
        if not configured:
            return await self.async_step_init()
        if user_input is not None:
            remaining = [d for d in configured if d not in user_input["remove"]]
            return self.async_create_entry(
                title="", data={**self._entry.options, CONF_DEVICES: remaining}
            )
        return self.async_show_form(
            step_id="remove",
            data_schema=vol.Schema(
                {vol.Required("remove", default=[]): cv.multi_select({d: d for d in configured})}
            ),
        )

    async def async_step_done(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        return self.async_create_entry(title="", data=dict(self._entry.options))

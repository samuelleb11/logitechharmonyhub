"""Thin async client for the Harmony IR appliance REST API."""
from __future__ import annotations

import asyncio

from aiohttp import ClientSession

from .const import DEFAULT_CARRIER, DEFAULT_SELECT


class ApiError(Exception):
    """Raised on any appliance API failure."""


class ApiClient:
    """Talks to the appliance's HTTP API (see docs: POST /api/ir/send, GET /api/ir/*)."""

    def __init__(self, session: ClientSession, host: str) -> None:
        self._session = session
        self._base = f"http://{host}"

    async def _get(self, path: str, timeout: float = 10) -> dict:
        try:
            async with asyncio.timeout(timeout):
                async with self._session.get(self._base + path) as resp:
                    resp.raise_for_status()
                    return await resp.json()
        except Exception as err:  # noqa: BLE001
            raise ApiError(str(err)) from err

    async def _post(self, path: str, payload: dict, timeout: float = 10) -> dict:
        try:
            async with asyncio.timeout(timeout):
                async with self._session.post(self._base + path, json=payload) as resp:
                    resp.raise_for_status()
                    return await resp.json()
        except Exception as err:  # noqa: BLE001
            raise ApiError(str(err)) from err

    async def status(self) -> dict:
        return await self._get("/api/status")

    async def send(self, device: str, function: str, select: int = DEFAULT_SELECT) -> dict:
        return await self._post(
            "/api/ir/send", {"device": device, "function": function, "select": select}
        )

    async def send_raw(
        self, raw_us: list[int], carrier: int = DEFAULT_CARRIER, select: int = DEFAULT_SELECT
    ) -> dict:
        return await self._post(
            "/api/ir/send", {"raw_us": raw_us, "carrier": carrier, "select": select}
        )

    async def ac_send(
        self,
        power: bool,
        mode: str,
        fan: str,
        temp: int,
        select: int = DEFAULT_SELECT,
    ) -> dict:
        """Drive a Midea/Danby AC from a climate state (encoded on the appliance)."""
        return await self._post(
            "/api/ac/send",
            {
                "power": "on" if power else "off",
                "mode": mode,
                "fan": fan,
                "temp": temp,
                "select": select,
            },
        )

    async def learn(self, timeout_ms: int = 15000) -> dict:
        """Capture the next remote button. Blocks on the appliance until a press or timeout."""
        return await self._post("/api/ir/learn", {"timeout_ms": timeout_ms}, timeout=timeout_ms / 1000 + 10)

    async def learn_save(
        self,
        function: str,
        carrier: int,
        us: list[int],
        device: str | None = None,
        model: str | None = None,
        dtype: str | None = None,
        brand: str | None = None,
    ) -> dict:
        """Persist a learned code as a custom device; returns {device, function}."""
        payload: dict = {"function": function, "carrier": carrier, "us": us}
        if device:
            payload["device"] = device
        if model:
            payload["model"] = model
        if dtype:
            payload["type"] = dtype
        if brand:
            payload["brand"] = brand
        return await self._post("/api/ir/learn/save", payload)

    async def forget(self, device: str, function: str | None = None) -> dict:
        """Delete a learned code (or the whole learned device if function is None)."""
        payload: dict = {"device": device}
        if function:
            payload["function"] = function
        return await self._post("/api/ir/forget", payload)

    # --- browse (handy for future entity/config UIs) ---
    async def types(self) -> list[str]:
        return (await self._get("/api/ir/types")).get("types", [])

    async def brands(self, dtype: str) -> list[str]:
        return (await self._get(f"/api/ir/brands?type={dtype}")).get("brands", [])

    async def devices(self, dtype: str | None = None, brand: str | None = None) -> list[dict]:
        query = []
        if dtype:
            query.append(f"type={dtype}")
        if brand:
            query.append(f"brand={brand}")
        qs = ("?" + "&".join(query)) if query else ""
        return (await self._get("/api/ir/devices" + qs)).get("devices", [])

    async def functions(self, device: str) -> list[str]:
        return (await self._get(f"/api/ir/functions?device={device}")).get("functions", [])

import httpx
import httpx as h
from httpx import Client as C


class MyClient(httpx.Client):
    def send(self, request, **kwargs):
        return super().send(request, **kwargs)


def via_alias(req: h.Request) -> h.Response:
    c = h.Client()
    return c.send(req)


def via_from_alias(req: httpx.Request) -> httpx.Response:
    return C().send(req)


def via_param(client: httpx.Client, req: httpx.Request) -> httpx.Response:
    return client.send(req)


def via_subclass(req: httpx.Request) -> httpx.Response:
    m = MyClient()
    return m.send(req)


def via_with(req: httpx.Request) -> None:
    with httpx.Client() as client:
        s = "é😀"; r = client.send(req)


async def decoy_async(req: httpx.Request) -> None:
    async with httpx.AsyncClient() as ac:
        await ac.send(req)

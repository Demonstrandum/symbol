#!/usr/bin/env python3
from __future__ import annotations

import asyncio
import importlib.util
import json
import os
import pathlib
import sys
from dataclasses import dataclass

ROOT = pathlib.Path(__file__).resolve().parents[3]


def generated_api() -> pathlib.Path:
    generated_dir = os.environ.get("SYMBOL_GENERATED_DIR")
    if generated_dir:
        path = pathlib.Path(generated_dir) / "api.py"
        if path.is_file():
            return path
    candidates = sorted(
        ROOT.glob("target/debug/build/symbol-*/out/api.py"),
        key=lambda path: path.stat().st_mtime_ns,
    )
    if not candidates:
        raise AssertionError("generated api.py is missing; build symbol first")
    return candidates[-1]


def load_api():
    spec = importlib.util.spec_from_file_location("symbol_api", generated_api())
    if spec is None or spec.loader is None:
        raise AssertionError("cannot load generated api.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


api = load_api()
IDENTITY = (
    ("Symbol-API-Version", api.API_VERSION),
    ("Symbol-API-Revision", str(api.API_REVISION)),
    ("Symbol-API-Source-Hash", api.SOURCE_HASH),
)


@dataclass
class Recorded:
    method: str
    url: str
    headers: tuple[tuple[str, str], ...]
    body: bytes | None


class Backend:
    def __init__(self) -> None:
        self.requests: list[Recorded] = []
        self.responses: list[api.ApiResponse] = []
        self.closed = 0

    def queue(
        self,
        status: int,
        body: object = b"",
        headers: tuple[tuple[str, str], ...] = (),
    ) -> None:
        encoded = (
            json.dumps(body, separators=(",", ":")).encode()
            if not isinstance(body, bytes)
            else body
        )
        self.responses.append(api.ApiResponse(status, IDENTITY + headers, encoded))

    def request(self, request: api.ApiRequest) -> api.ApiResponse:
        body = api._body(request.body)
        self.requests.append(
            Recorded(str(request.method), request.url, request.headers, body)
        )
        if not self.responses:
            raise AssertionError(f"unexpected request {request.method} {request.url}")
        return self.responses.pop(0)

    def close(self) -> None:
        self.closed += 1


class AsyncBackend:
    def __init__(self, response: api.ApiResponse) -> None:
        self.response = response
        self.requests: list[api.ApiRequest] = []
        self.closed = 0

    async def request(self, request: api.ApiRequest) -> api.ApiResponse:
        self.requests.append(request)
        return self.response

    async def close(self) -> None:
        self.closed += 1


def mutation_headers(path: str = "x.txt") -> tuple[tuple[str, str], ...]:
    return (
        ("Location", f"http://symbol/demo/{path}"),
        ("ETag", '"tree"'),
        ("Content-Revision", "2"),
        ("Undo-Token", "undo"),
        ("Undo-Expires", "2026-08-27T00:00:00Z"),
    )


def test_metadata_and_media_types() -> None:
    assert api.API_VERSION.count(".") == 2
    assert api.API_REVISION > 0
    assert len(api.SOURCE_HASH) == 64
    assert str(api.MediaTypes.JSON) == "application/json"
    assert str(api.MediaType.text("plain")) == "text/plain; charset=utf-8"
    assert api.ContentFormats.HTML.extensions == ("html", "htm")
    assert api.MediaType.parse('text/plain; charset="utf-8"') == api.MediaTypes.TEXT


def test_sync_client_mapping() -> None:
    backend = Backend()
    client = api.HttpClient.wrap(backend)
    symbol = api.Symbol(client, origin="http://symbol")

    backend.queue(
        200,
        {
            "sites": 1,
            "files": 2,
            "aliases": 0,
            "blobs": 2,
            "bytes": 3,
            "logical_bytes": 3,
            "saved_bytes": 0,
            "saved_fraction": 0.0,
        },
        (("Content-Type", "application/json"),),
    )
    assert symbol.stats().files == 2

    backend.queue(200, b"export {}", (("Content-Type", "text/typescript"),))
    assert symbol.api_client(api.ApiClientAsset.TYPESCRIPT).status == 200
    assert backend.requests[-1].url == "http://symbol/api.ts"

    backend.queue(200, ("a" * 64 + "\n").encode())
    assert symbol.api_client_hash(api.ApiClientAsset.TYPESCRIPT) == "a" * 64

    backend.queue(200, b"# TypeScript", (("Content-Type", "text/markdown"),))
    assert symbol.api_manual(api.ApiManual.TYPESCRIPT).text() == "# TypeScript"

    backend.queue(
        200,
        {
            "api_version": api.API_VERSION,
            "absolute_revision": api.API_REVISION,
            "source_hash": api.SOURCE_HASH,
        },
        (("Content-Type", "application/json"),),
    )
    assert symbol.api_version().source_hash == api.SOURCE_HASH

    backend.queue(200, b"hello", (("Content-Type", "text/plain"),))
    assert symbol.site("demo").file("space name.txt").text() == "hello"
    assert backend.requests[-1].url.endswith("/demo/space%20name.txt")

    backend.queue(200, b"ok", mutation_headers())
    receipt = symbol.site("demo").file("x.txt").put("value", api.MediaTypes.TEXT)
    assert receipt.changed and receipt.undo is not None
    assert not any(
        name == "Idempotency-Key" for name, _ in backend.requests[-1].headers
    )

    backend.queue(
        201,
        {
            "path": "note/hash.anno",
            "target": "target",
            "target_kind": "file",
            "dangling": False,
        },
        mutation_headers("note/hash.anno"),
    )
    symbol.site("demo").alias("note/hash.anno", "target")
    request = backend.requests[-1]
    assert request.method == "ALIAS"
    assert any(name == "Idempotency-Key" for name, _ in request.headers)

    client.close()
    assert backend.closed == 0  # wrapped backends are borrowed


def test_allocation_naming() -> None:
    backend = Backend()
    symbol = api.Symbol(api.HttpClient.wrap(backend), origin="http://symbol")
    backend.queue(
        201,
        {
            "site": "demo",
            "path": "notes/pre-hash-final.anno",
            "hash": "hash",
            "blob_url": "/.blob/demo/hash",
        },
        mutation_headers("notes/pre-hash-final.anno"),
    )
    options = api.CreateFileOptions(
        name=api.GeneratedName(prefix="pre-", suffix="-final", extension=".anno"),
        media_type=api.MediaTypes.BINARY,
    )
    symbol.site("demo").folder("notes").bytes(b"opaque", options)
    headers = dict(backend.requests[-1].headers)
    assert headers["File-Prefix"] == "pre-"
    assert headers["File-Suffix"] == "-final"
    assert headers["File-Extension"] == "anno"


async def test_async_factory() -> None:
    response = api.ApiResponse(
        200,
        IDENTITY + (("Content-Type", "application/json"),),
        b'{"sites":0,"files":0,"aliases":0,"blobs":0,"bytes":0,"logical_bytes":0,"saved_bytes":0,"saved_fraction":0}',
    )
    backend = AsyncBackend(response)
    client = api.AsyncHttpClient.wrap(backend)
    symbol = api.Symbol(client, origin="http://symbol")
    assert (await symbol.stats()).sites == 0
    backend.response = api.ApiResponse(
        200,
        IDENTITY + (("Content-Type", "text/plain"),),
        b"async",
    )
    assert await symbol.site("demo").file("x.txt").text() == "async"
    await client.close()
    assert backend.closed == 0


def main() -> None:
    test_metadata_and_media_types()
    test_sync_client_mapping()
    test_allocation_naming()
    asyncio.run(test_async_factory())
    print("SDK Python runtime: 4 tests")


if __name__ == "__main__":
    main()

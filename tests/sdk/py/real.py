#!/usr/bin/env python3
from __future__ import annotations

import asyncio
import importlib.util
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[3]


def load_api():
    generated_dir = os.environ.get("SYMBOL_GENERATED_DIR")
    if generated_dir:
        candidates = [pathlib.Path(generated_dir) / "api.py"]
    else:
        candidates = sorted(
            ROOT.glob("target/debug/build/symbol-*/out/api.py"),
            key=lambda path: path.stat().st_mtime_ns,
        )
    path = candidates[-1]
    spec = importlib.util.spec_from_file_location("symbol_api_real", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def port() -> int:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    value = listener.getsockname()[1]
    listener.close()
    return value


api = load_api()
listen = port()
origin = f"http://127.0.0.1:{listen}"
symbol_bin = pathlib.Path(os.environ.get("SYMBOL_BIN", ROOT / "target/debug/symbol"))

with tempfile.TemporaryDirectory() as root:
    process = subprocess.Popen(
        [
            str(symbol_bin),
            "--bind",
            f"127.0.0.1:{listen}",
            "--root",
            root,
        ],
        env={
            "PATH": "/usr/bin:/bin",
            "SYMBOL_ALLOW_DEV_ORIGIN": "true",
            "RUST_LOG": "warn",
        },
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        for _ in range(100):
            try:
                urllib.request.urlopen(origin + "/STATS", timeout=0.2).read()
                break
            except OSError:
                time.sleep(0.05)
        else:
            raise AssertionError("temporary Symbol server did not start")

        symbol = api.Symbol(origin=origin)
        assert symbol.api_client(api.ApiClientAsset.PYTHON).status == 200
        assert len(symbol.api_client_hash(api.ApiClientAsset.PYTHON)) == 64
        assert symbol.api_manual(api.ApiManual.PYTHON).status == 200
        assert symbol.api_version().api_version == api.API_VERSION_PARTS
        assert symbol.sites().entries[0].kind is api.DirectoryKind.BUILTIN
        created = (
            symbol.site("python-sdk")
            .file("index.html")
            .put("<h1>python</h1>", api.MediaTypes.HTML)
        )
        assert created.status == 201
        upload_source = pathlib.Path(root) / "streamed-upload.bin"
        upload_source.write_bytes(b"streamed" * 131_072)
        symbol.site("python-sdk").file("streamed-upload.bin").put(upload_source)
        assert symbol.site("python-sdk").file("index.html").text() == "<h1>python</h1>"
        inventory = symbol.site("python-sdk").files()
        assert inventory.site == "python-sdk"
        assert inventory.files[0].path == "index.html"

        allocated = (
            symbol.site("python-sdk")
            .folder("notes")
            .bytes(
                b"opaque",
                api.CreateFileOptions(
                    name=api.GeneratedName(extension="anno"),
                    media_type=api.MediaTypes.BINARY,
                ),
            )
        )
        assert "/python-sdk/notes/" in allocated.mutation.location

        alias = symbol.site("python-sdk").alias("latest", "index.html")
        assert alias.mutation.status == 201
        assert symbol.site("python-sdk").file("latest").text() == "<h1>python</h1>"

        for index, client in enumerate(
            (
                api.HttpClient.requests(),
                api.HttpClient.urllib3(),
                api.HttpClient.httpx(),
            )
        ):
            with api.Symbol(client, origin=origin) as backend_symbol:
                assert backend_symbol.stats().sites >= 1
                sync_path = f"backend-sync-{index}.bin"
                backend_symbol.site("python-sdk").file(sync_path).put(upload_source)
                response = backend_symbol.site("python-sdk").file("index.html").get()
                assert b"".join(response.iter_bytes(4)) == b"<h1>python</h1>"

        async def check_async_backends() -> None:
            for index, client in enumerate(
                (
                    api.AsyncHttpClient.httpx(),
                    api.AsyncHttpClient.aiohttp(),
                )
            ):
                async with api.Symbol(client, origin=origin) as backend_symbol:
                    assert (await backend_symbol.stats()).sites >= 1

                    async def upload_chunks():
                        yield upload_source.read_bytes()

                    await (
                        backend_symbol.site("python-sdk")
                        .file(f"backend-async-{index}.bin")
                        .put(upload_chunks())
                    )
                    response = await (
                        backend_symbol.site("python-sdk").file("index.html").get()
                    )
                    chunks = bytearray()
                    async for chunk in response.aiter_bytes(4):
                        chunks.extend(chunk)
                    assert chunks == b"<h1>python</h1>"

        asyncio.run(check_async_backends())

        async def check_async_hierarchy() -> None:
            async with api.Symbol(
                api.AsyncHttpClient.httpx(),
                origin=origin,
            ) as async_symbol:
                site = async_symbol.site("python-sdk")
                assert (await async_symbol.sites()).entries[
                    0
                ].kind is api.DirectoryKind.BUILTIN
                assert (await site.files()).site == "python-sdk"
                scratch = site.file("scratch.bin")
                await scratch.put(b"abcdef")
                base_hash = await scratch.hash()
                await scratch.replace(b"uvwxyz", base_hash=base_hash)
                base_hash = await scratch.hash()
                await scratch.splice(
                    api.ByteSplice(1, 2, b"12"),
                    base_hash=base_hash,
                )
                assert await scratch.bytes() == b"u12xyz"
                await scratch.remove()
                await site.folder("generated").json({"async": True})

                async def choose_name(proposal):
                    assert proposal.hash.startswith("blake3:")
                    return "custom-async.bin"

                await site.folder("generated").create(
                    b"custom",
                    api.CreateFileOptions(name=choose_name),
                )
                await site.alias("async-latest", "index.html")
                await site.aliases((api.AliasDefinition("async-copy", "async-latest"),))
                assert (await site.undo_stack()).entries
                assert (await site.expiry()).site == "python-sdk"
                assert not (await site.management().status()).managed
                await site.copy("python-sdk-copy")
                moved = async_symbol.site("python-sdk-copy")
                await moved.move("python-sdk-moved")
                archive = await async_symbol.site("python-sdk-moved").pop()
                await archive.aclose()

        asyncio.run(check_async_hierarchy())
        print("SDK Python real temporary workflow: ok")
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

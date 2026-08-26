#!/usr/bin/env python3
from __future__ import annotations

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
        created = (
            symbol.site("python-sdk")
            .file("index.html")
            .put("<h1>python</h1>", api.MediaTypes.HTML)
        )
        assert created.status == 201
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
        print("SDK Python real temporary workflow: ok")
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

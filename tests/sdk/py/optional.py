#!/usr/bin/env python3
from __future__ import annotations

import importlib.abc
import importlib.util
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
BLOCKED = {"aiohttp", "httpx", "requests", "urllib3"}


class Blocker(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname: str, path=None, target=None):
        if fullname.partition(".")[0] in BLOCKED:
            raise ImportError(f"blocked optional package {fullname}")
        return None


finder = Blocker()
sys.meta_path.insert(0, finder)
try:
    generated_dir = os.environ.get("SYMBOL_GENERATED_DIR")
    generated = (
        pathlib.Path(generated_dir) / "symbol.py"
        if generated_dir
        else sorted(
            ROOT.glob("target/debug/build/symbol-*/out/symbol.py"),
            key=lambda path: path.stat().st_mtime_ns,
        )[-1]
    )
    spec = importlib.util.spec_from_file_location("symbol_api_optional", generated)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)

    client = module.HttpClient.stdlib()
    client.close()
    for factory in (
        module.HttpClient.requests,
        module.HttpClient.urllib3,
        module.HttpClient.httpx,
        module.AsyncHttpClient.httpx,
        module.AsyncHttpClient.aiohttp,
    ):
        try:
            factory()
        except module.MissingOptionalDependency:
            pass
        else:
            raise AssertionError(f"{factory.__qualname__} did not fail lazily")
finally:
    sys.meta_path.remove(finder)

print("SDK Python optional imports: ok")

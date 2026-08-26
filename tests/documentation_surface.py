#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
API = (ROOT / "API.md").read_text()
HOMEPAGE = (ROOT / "static/docs.md").read_text()
SYMBOL = pathlib.Path(os.environ.get("SYMBOL_BIN", ROOT / "target/debug/symbol"))

SECTIONS = ("INDEX", "JS", "PYTHON", "SHELL", "PROTOCOL")
JS_METHODS = (
    "request",
    "docs",
    "docsHash",
    "installer",
    "installerHash",
    "shellClient",
    "shellClientHash",
    "apiClient",
    "apiClientHash",
    "apiManual",
    "apiVersion",
    "stats",
    "sites",
    "create",
    "site",
    "blob",
    "assertExactApi",
    "file",
    "folder",
    "redirect",
    "get",
    "put",
    "remove",
    "archive",
    "pop",
    "files",
    "alias",
    "aliases",
    "copy",
    "move",
    "undo",
    "undoStack",
    "setExpiry",
    "expiry",
    "management",
    "bytes",
    "text",
    "json",
    "html",
    "replace",
    "splice",
    "patch",
    "hash",
    "status",
    "claim",
    "rotate",
    "release",
    "retry",
    "abort",
)
PYTHON_METHODS = (
    "stats",
    "sites",
    "site",
    "api_client",
    "api_client_hash",
    "api_manual",
    "api_version",
    "request",
    "file",
    "folder",
    "files",
    "alias",
    "aliases",
    "copy",
    "move",
    "archive",
    "pop",
    "undo",
    "create",
    "bytes",
    "text",
    "json",
    "get",
    "put",
    "remove",
    "hash",
    "replace",
    "splice",
)
SHELL_COMMANDS = (
    "put",
    "clone",
    "get",
    "pop",
    "copy",
    "remix",
    "move",
    "alias",
    "sync",
    "undo",
    "expire",
    "manage",
    "recover",
    "ls",
    "rm",
    "url",
    "stats",
    "update",
    "help",
)
SHELL_ALIASES = (
    ("push", "put"),
    ("pull", "clone"),
    ("rename", "move"),
    ("add", "put"),
    ("x", "remix"),
    ("list", "ls"),
    ("download", "get"),
    ("delete", "rm"),
    ("upgrade", "update"),
)
SDK_ASSETS = ("api.ts", "api.js", "api.global.js", "api.d.ts", "api.py", "symbol.sh")


def marked_section(name: str) -> str:
    start = f"<!-- API:{name}:START -->"
    end = f"<!-- API:{name}:END -->"
    assert API.count(start) == 1, start
    assert API.count(end) == 1, end
    return API.split(start, 1)[1].split(end, 1)[0]


sections = tuple(marked_section(name) for name in SECTIONS)
index, javascript, python, shell, protocol = sections

for method in JS_METHODS:
    assert f"`{method}`" in javascript, f"undocumented JavaScript method: {method}"
for method in PYTHON_METHODS:
    assert f"`{method}`" in python, f"undocumented Python method: {method}"
for command in SHELL_COMMANDS:
    assert f"`{command}`" in shell, f"undocumented shell command: {command}"
for alias, command in SHELL_ALIASES:
    assert f"`{alias}` → `{command}`" in shell, f"undocumented shell alias: {alias}"
for asset in SDK_ASSETS:
    assert f"`/{asset}`" in index or f"`{asset}`" in index, asset

contract = json.loads(subprocess.check_output([SYMBOL, "contract"], text=True))
for endpoint in contract:
    marker = f"<!-- contract:{endpoint['name']} -->"
    assert protocol.count(marker) == 1, f"contract marker mismatch: {endpoint['name']}"

for manual in ("/API/JS", "/API/PY", "/API/SH", "/API/CURL"):
    assert manual in HOMEPAGE, f"homepage omitted manual link {manual}"
assert "curl -X " not in HOMEPAGE
assert "<!-- contract:" not in HOMEPAGE

print(
    f"documentation surface: {len(contract)} endpoints, "
    f"{len(JS_METHODS)} JavaScript methods, {len(PYTHON_METHODS)} Python methods, "
    f"{len(SHELL_COMMANDS)} shell commands"
)

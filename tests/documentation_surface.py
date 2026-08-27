#!/usr/bin/env python3
from __future__ import annotations

import ast
import json
import os
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
API = (ROOT / "API.md").read_text()
HOMEPAGE = (ROOT / "static/docs.md").read_text()
SYMBOL = pathlib.Path(os.environ.get("SYMBOL_BIN", ROOT / "target/debug/symbol"))

SECTIONS = ("INDEX", "JS", "PYTHON", "SHELL", "PROTOCOL")
SDK_ASSETS = (
    "symbol.ts",
    "symbol.js",
    "symbol.global.js",
    "symbol.d.ts",
    "symbol.py",
    "symbol.sh",
)


def generated_file(name: str) -> pathlib.Path:
    generated = os.environ.get("SYMBOL_GENERATED_DIR")
    if generated is not None:
        return pathlib.Path(generated) / name
    candidates = sorted(
        ROOT.glob(f"target/debug/build/symbol-*/out/{name}"),
        key=lambda path: path.stat().st_mtime_ns,
    )
    assert candidates, f"generated {name} is missing"
    return candidates[-1]


def typescript_methods() -> tuple[str, ...]:
    declarations = generated_file("symbol.d.ts").read_text()
    methods: set[str] = set()
    for matched in re.finditer(r"export declare class \w+[^{]*\{", declarations):
        start = matched.end()
        depth = 1
        at = start
        while depth:
            match declarations[at]:
                case "{":
                    depth += 1
                case "}":
                    depth -= 1
            at += 1
        body = declarations[start : at - 1]
        methods.update(
            method
            for method in re.findall(
                r"^\s+(?:static\s+)?(?:get\s+)?([A-Za-z]\w*)\s*\(",
                body,
                re.MULTILINE,
            )
            if method != "constructor"
        )
    return tuple(sorted(methods))


def python_methods() -> tuple[str, ...]:
    tree = ast.parse((ROOT / "static/api.py").read_text())
    methods: set[str] = set()
    for statement in tree.body:
        if not isinstance(statement, ast.ClassDef):
            continue
        methods.update(
            member.name
            for member in statement.body
            if isinstance(member, ast.FunctionDef | ast.AsyncFunctionDef)
            and not member.name.startswith("_")
        )
    return tuple(sorted(methods))


def shell_surface() -> tuple[tuple[str, ...], tuple[tuple[str, str], ...]]:
    output = subprocess.check_output(
        [ROOT / "static/symbol.sh"],
        env={**os.environ, "SYMBOL_TEST_COMMAND_REGISTRY": "1"},
        text=True,
    )
    commands: list[str] = []
    aliases: list[tuple[str, str]] = []
    for line in output.splitlines():
        canonical, *spellings = line.split()
        commands.append(canonical)
        aliases.extend(
            (spelling, canonical)
            for spelling in spellings
            if spelling != canonical and not spelling.startswith("-")
        )
    return tuple(commands), tuple(aliases)


def marked_section(name: str) -> str:
    start = f"<!-- API:{name}:START -->"
    end = f"<!-- API:{name}:END -->"
    assert API.count(start) == 1, start
    assert API.count(end) == 1, end
    return API.split(start, 1)[1].split(end, 1)[0]


sections = tuple(marked_section(name) for name in SECTIONS)
index, javascript, python, shell, protocol = sections
js_methods = typescript_methods()
py_methods = python_methods()
shell_commands, shell_aliases = shell_surface()

for manual, minimum_lines, headings in (
    (
        javascript,
        300,
        (
            "## Installation and module formats",
            "## SymbolClient reference",
            "## SiteClient reference",
            "## FolderClient reference",
            "## FileClient reference",
            "## Receipts and errors",
            "## Complete workflow",
        ),
    ),
    (
        python,
        300,
        (
            "## Installation and imports",
            "## Synchronous client reference",
            "## Asynchronous client reference",
            "## Transport backends",
            "## Models and errors",
            "## Complete workflow",
        ),
    ),
    (
        shell,
        250,
        (
            "## Installation and configuration",
            "## Command reference",
            "## Checkout and sync workflow",
            "## Management and recovery",
            "## Complete workflow",
        ),
    ),
):
    assert len(manual.splitlines()) >= minimum_lines, "API manual is only a summary"
    for heading in headings:
        assert heading in manual, f"API manual omitted {heading}"

missing_js = tuple(method for method in js_methods if f"`{method}`" not in javascript)
missing_python = tuple(method for method in py_methods if f"`{method}`" not in python)
assert not missing_js and not missing_python, (
    f"undocumented JavaScript methods: {missing_js}; "
    f"undocumented Python methods: {missing_python}"
)
for command in shell_commands:
    assert f"`{command}`" in shell, f"undocumented shell command: {command}"
for alias, command in shell_aliases:
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
    f"{len(js_methods)} JavaScript methods, {len(py_methods)} Python methods, "
    f"{len(shell_commands)} shell commands"
)

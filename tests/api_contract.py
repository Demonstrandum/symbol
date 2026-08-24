#!/usr/bin/env python3
import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
api = (root / "API.md").read_text()
main = (root / "src/main.rs").read_text()

required_sections = [
    "GET /",
    "PUT /",
    "GET /HASH",
    "GET /STATS",
    "GET /FILES",
    "GET /{name}",
    "GET /{name}/{path...}",
    "PUT /{name}",
    "PUT /{name}/{path...}",
    "DELETE /{name}",
    "DELETE /{name}/{path...}",
    "COPY /{name}",
    "MOVE /{name}",
    "GET /{name}/UNDO",
    "UNDO /{name}",
    "GET /{name}/EXPIRES",
    "EXPIRE /{name}",
    "MANAGE /{name}",
    "GET /.blob/{name}/{hash}",
]

missing = [section for section in required_sections if f"`{section}" not in api]
if missing:
    print("API.md is missing route sections:", ", ".join(missing), file=sys.stderr)
    sys.exit(1)

implemented_markers = [
    '.route("/",',
    '.route("/HASH"',
    '.route("/STATS"',
    '.route("/FILES"',
    '.route("/{name}/UNDO"',
    '.route("/{name}/EXPIRES"',
    '"COPY" =>',
    '"MOVE" =>',
    '"UNDO" =>',
    '"EXPIRE" =>',
    '"MANAGE" =>',
]
for marker in implemented_markers:
    if marker not in main:
        print(f"router contract marker missing from src/main.rs: {marker}", file=sys.stderr)
        sys.exit(1)

json_blocks = re.findall(r"```json\s*\n(.*?)\n```", api, re.DOTALL)
for index, block in enumerate(json_blocks, 1):
    try:
        json.loads(block)
    except json.JSONDecodeError as error:
        print(f"invalid API.md JSON block {index}: {error}", file=sys.stderr)
        sys.exit(1)

if "Current limitation" in api:
    print("API.md contains an unresolved implementation limitation", file=sys.stderr)
    sys.exit(1)

print(
    f"API contract: {len(required_sections)} route groups, "
    f"{len(json_blocks)} JSON examples"
)

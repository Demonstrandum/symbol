#!/usr/bin/env python3
import json
import pathlib
import re
import shlex
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
api = (root / "API.md").read_text()
contract = json.loads(
    subprocess.check_output([root / "target/debug/symbol", "contract"], text=True)
)

failures = []
for endpoint in contract:
    methods = endpoint["methods"]
    path = endpoint["path"].split(" | ", 1)[0]
    if path not in api:
        failures.append(f"undocumented path: {path}")
    for method in methods:
        if method not in {"HEAD"} and f"`{method} " not in api:
            failures.append(f"undocumented method: {method}")
    for header in endpoint["request_headers"] + endpoint["response_headers"]:
        if (
            f"`{header}`" not in api
            and f"`{header}:" not in api
            and header not in {
            "Content-Type",
            "Content-Length",
            "Cache-Control",
            "ETag",
            "Expires",
            "Location",
            }
        ):
            failures.append(f"undocumented header: {header}")
    for status in endpoint["success_statuses"]:
        if f"`{status}" not in api:
            failures.append(
                f"undocumented success status {status} for {endpoint['name']}"
            )

json_blocks = re.findall(r"```json\s*\n(.*?)\n```", api, re.DOTALL)
for index, block in enumerate(json_blocks, 1):
    try:
        json.loads(block)
    except json.JSONDecodeError as error:
        failures.append(f"invalid JSON block {index}: {error}")

shell_blocks = re.findall(r"```sh\s*\n(.*?)\n```", api, re.DOTALL)
curl_examples = []
for block in shell_blocks:
    logical = block.replace("\\\n", " ")
    for line in logical.splitlines():
        line = line.strip()
        if line.startswith("curl "):
            curl_examples.append(line)
            try:
                shlex.split(line)
            except ValueError as error:
                failures.append(f"invalid curl shell example: {error}: {line}")

if "Current limitation" in api:
    failures.append("API.md contains an unresolved implementation limitation")

if failures:
    print("\n".join(sorted(set(failures))), file=sys.stderr)
    sys.exit(1)

print(
    f"API contract: {len(contract)} generated route groups, "
    f"{len(json_blocks)} JSON examples, {len(curl_examples)} curl examples"
)

#!/usr/bin/env python3
import json
import pathlib
import re
import shlex
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
api = (root / "API.md").read_text()
freeze = json.loads((root / "public-api-freeze.json").read_text())
contract = json.loads(
    subprocess.check_output([root / "target/debug/symbol", "contract"], text=True)
)


def documented_section(name):
    marker = f"<!-- contract:{name} -->"
    marker_at = api.find(marker)
    if marker_at < 0:
        return None
    heading_at = api.find("### ", marker_at)
    if heading_at < 0:
        return None
    next_contract = api.find("\n<!-- contract:", heading_at + 4)
    return api[heading_at : next_contract if next_contract >= 0 else len(api)]


common = api[: api.find("## Route inventory")]
errors = api[api.find("## Status and error mapping") :]
failures = []
names = set()
normalized_api = re.sub(r"\s+", " ", api)
for required_reserved_rule in (
    "`FILES`, `UNDO`, and `EXPIRES` reserve their whole top-level virtual namespace",
    "`FILES`, `HASH`, `UNDO`, `EXPIRES`, `symbol.toml`, `.symbol-token`, and `.symbol-claim` are also reserved as the final component of any mutation path",
    "`POST`, `ALIAS`, `REPLACE`, `PATCH`, `PUT`, `DELETE`, and `EXPIRE` apply both rules uniformly",
    "`error: path is reserved by symbol\\n`",
    "Authentication failure takes precedence and returns `401`",
):
    if required_reserved_rule not in normalized_api:
        failures.append(
            f"missing exact reserved mutation rule: {required_reserved_rule}"
        )
for endpoint in contract:
    name = endpoint["name"]
    if name in names:
        failures.append(f"duplicate contract name: {name}")
    names.add(name)
    section = documented_section(name)
    if section is None:
        failures.append(f"missing API section marker for contract: {name}")
        continue
    method = endpoint["method"]
    if f"`{method} " not in section:
        failures.append(f"{name}: method {method} absent from matching section")
    for header in endpoint["request_headers"] + endpoint["response_headers"]:
        if (
            header not in section
            and f"`{header}`" not in common
            and f"{header}:" not in common
        ):
            failures.append(f"{name}: undocumented header {header}")
    local_status_text = section + errors
    for status in endpoint["success_statuses"] + endpoint["error_statuses"]:
        if not re.search(rf"(?:`|HTTP/1\.1 ){status}\b", local_status_text):
            failures.append(f"{name}: undocumented status {status}")

json_blocks = re.findall(r"```json\s*\n(.*?)\n```", api, re.DOTALL)
for index, block in enumerate(json_blocks, 1):
    try:
        json.loads(block)
    except json.JSONDecodeError as error:
        failures.append(f"invalid JSON block {index}: {error}")

http_blocks = re.findall(r"```http\s*\n(.*?)\n```", api, re.DOTALL)
for index, block in enumerate(http_blocks, 1):
    first = block.splitlines()[0]
    if not (
        re.match(r"^[A-Z]+ \S+ HTTP/1\.1$", first)
        or re.match(r"^HTTP/1\.1 [1-5][0-9][0-9]\b", first)
        or re.match(r"^[A-Za-z0-9-]+:", first)
    ):
        failures.append(f"invalid raw HTTP example {index}: {first}")

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
    f"API contract: {len(contract)} method-specific endpoints, "
    f"{len(json_blocks)} JSON, {len(http_blocks)} HTTP, "
    f"{len(curl_examples)} curl examples"
)

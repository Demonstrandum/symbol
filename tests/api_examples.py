#!/usr/bin/env python3
from __future__ import annotations

import dataclasses
import http.client
import io
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.parse


@dataclasses.dataclass(frozen=True)
class HttpExample:
    request_line: str
    expected_status: int


@dataclasses.dataclass(frozen=True)
class ObservedResponse:
    status: int
    headers: tuple[tuple[str, str], ...]
    body: bytes

    def header(self, name: str) -> str:
        folded = name.casefold()
        return next(value for key, value in self.headers if key.casefold() == folded)


ROOT = pathlib.Path(__file__).resolve().parent.parent
API = (ROOT / "API.md").read_text()
BASE = sys.argv[1].rstrip("/")
PARSED_BASE = urllib.parse.urlsplit(BASE)
if PARSED_BASE.scheme != "http" or not PARSED_BASE.hostname:
    raise SystemExit(f"temporary test server must use an http URL, got {BASE!r}")


def request(
    method: str,
    path: str,
    body: bytes = b"",
    headers: tuple[tuple[str, str], ...] = (),
) -> ObservedResponse:
    connection = http.client.HTTPConnection(PARSED_BASE.hostname, PARSED_BASE.port, timeout=10)
    connection.request(method, path, body=body, headers=dict(headers))
    response = connection.getresponse()
    observed = ObservedResponse(response.status, tuple(response.getheaders()), response.read())
    connection.close()
    return observed


def require_status(label: str, observed: ObservedResponse, expected: int) -> None:
    if observed.status != expected:
        raise AssertionError(
            f"{label}: expected HTTP {expected}, observed {observed.status}: "
            f"{observed.body.decode(errors='replace')}"
        )


def gzip_site() -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        payload = b"<h1>raw merge</h1>\n"
        info = tarfile.TarInfo("index.html")
        info.size = len(payload)
        archive.addfile(info, io.BytesIO(payload))
    return output.getvalue()


def execute_raw_http(example: HttpExample, managed_token: str, etag: str) -> None:
    if example.request_line == "PUT / HTTP/1.1":
        observed = request(
            "PUT",
            "/",
            b"<h1>Hello</h1>",
            (
                ("Content-Type", "text/html"),
                ("Idempotency-Key", "deploy-2026-08-20"),
            ),
        )
    elif example.request_line == "GET /hello/FILES HTTP/1.1":
        observed = request(
            "GET",
            "/hello/FILES",
            headers=(("Accept", "application/json"),),
        )
    elif example.request_line == "PUT /hello HTTP/1.1":
        payload = gzip_site()
        observed = request(
            "PUT",
            "/api-raw",
            payload,
            (
                ("Content-Type", "application/gzip"),
                ("Content-Disposition", 'attachment; filename="site.tar.gz"'),
                ("Unpack", "1"),
                ("If-Match", etag),
                ("Authorization", f"Bearer {managed_token}"),
            ),
        )
    else:
        raise AssertionError(f"unmapped normative raw HTTP request: {example.request_line}")
    require_status(example.request_line, observed, example.expected_status)


def raw_http_examples() -> tuple[HttpExample, ...]:
    expected = {
        "PUT / HTTP/1.1": 201,
        "GET /hello/FILES HTTP/1.1": 200,
        "PUT /hello HTTP/1.1": 200,
    }
    found: list[HttpExample] = []
    for block in re.findall(r"```http\s*\n(.*?)\n```", API, re.DOTALL):
        first = block.splitlines()[0]
        if re.fullmatch(r"[A-Z]+ \S+ HTTP/1\.1", first):
            if first not in expected:
                raise AssertionError(f"raw HTTP request lacks an executable probe: {first}")
            found.append(HttpExample(first, expected[first]))
    if {example.request_line for example in found} != set(expected):
        raise AssertionError("not every normative raw HTTP request example was discovered")
    return tuple(found)


def curl_examples() -> tuple[str, ...]:
    commands: list[str] = []
    for block in re.findall(r"```sh\s*\n(.*?)\n```", API, re.DOTALL):
        logical = block.replace("\\\n", " ")
        commands.extend(
            line.strip() for line in logical.splitlines() if line.strip().startswith("curl ")
        )
    if not commands:
        raise AssertionError("API.md has no executable curl examples")
    return tuple(commands)


claim = "sym_claim_" + "01" * 32
created = request(
    "PUT",
    "/api-raw/index.html",
    b"<h1>before raw merge</h1>\n",
    (("Management-Action", "claim"), ("Creator-Claim", claim)),
)
require_status("raw merge fixture", created, 201)
token = created.header("Management-Token")
inventory = request(
    "GET",
    "/api-raw/FILES",
    headers=(("Accept", "application/json"),),
)
require_status("raw merge fixture inventory", inventory, 200)
baseline = inventory.header("ETag")

examples = raw_http_examples()
for raw_example in examples:
    execute_raw_http(raw_example, token, baseline)

commands = curl_examples()
with tempfile.TemporaryDirectory() as work:
    environment = os.environ.copy()
    environment["SYMBOL_BASE"] = BASE
    for command in commands:
        subprocess.run(command, shell=True, check=True, cwd=work, env=environment)
    archive = pathlib.Path(work, "hello.tar.gz")
    with tarfile.open(archive, "r:gz") as packaged:
        if "symbol.toml" not in packaged.getnames():
            raise AssertionError("documented curl archive omitted symbol.toml")

require_status("documented curl deletion", request("GET", "/hello/"), 404)
print(
    f"API examples: executed {len(examples)} raw HTTP requests and "
    f"{len(commands)} curl commands"
)

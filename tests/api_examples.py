#!/usr/bin/env python3
from __future__ import annotations

import dataclasses
import http.client
import io
import json
import os
import pathlib
import re
import struct
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


def require_readable_location(label: str, observed: ObservedResponse) -> None:
    try:
        location = observed.header("Location")
    except StopIteration:
        return
    target = urllib.parse.urlsplit(location)
    fetched = request("GET", target.path or "/", headers=())
    if not 200 <= fetched.status < 400:
        raise AssertionError(
            f"{label} returned unreadable Location {location!r}: {fetched.status}"
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
    method, path, protocol = example.request_line.split()
    if protocol != "HTTP/1.1":
        raise AssertionError(f"unsupported documented HTTP version: {protocol}")
    if example.request_line == "PUT / HTTP/1.1":
        observed = request(
            method,
            path,
            b"<h1>Hello</h1>",
            (
                ("Content-Type", "text/html"),
                ("Idempotency-Key", "deploy-2026-08-20"),
            ),
        )
    elif example.request_line == "GET /hello/FILES HTTP/1.1":
        observed = request(
            method,
            path,
            headers=(("Accept", "application/json"),),
        )
    elif example.request_line == "PUT /hello HTTP/1.1":
        payload = gzip_site()
        observed = request(
            method,
            path,
            payload,
            (
                ("Content-Type", "application/gzip"),
                ("Content-Disposition", 'attachment; filename="site.tar.gz"'),
                ("Unpack", "1"),
                ("If-Match", etag),
                ("Authorization", f"Bearer {managed_token}"),
            ),
        )
        require_status(example.request_line, observed, example.expected_status)
        unpacked = request("GET", "/hello/index.html")
        if unpacked.status == 307:
            unpacked = request("GET", urllib.parse.urlsplit(unpacked.header("Location")).path)
        require_status("documented PUT /hello state", unpacked, 200)
        if unpacked.body != b"<h1>raw merge</h1>\n":
            raise AssertionError("documented PUT /hello did not update exact /hello state")
        return
    elif example.request_line == "ALIAS /hello/current HTTP/1.1":
        current_etag = request(
            "GET",
            "/hello/FILES",
            headers=(("Accept", "application/json"),),
        ).header("ETag")
        observed = request(
            method,
            path,
            headers=(
                ("Alias-Target", "index.html"),
                ("If-Match", current_etag),
                ("Idempotency-Key", "alias-current-v1"),
                ("Authorization", f"Bearer {managed_token}"),
            ),
        )
    elif example.request_line == "REPLACE /hello/data.bin HTTP/1.1":
        fixture = request(
            "PUT",
            path,
            b"before",
            (("Authorization", f"Bearer {managed_token}"),),
        )
        require_status("raw REPLACE fixture", fixture, 200)
        content_hash = request("GET", f"{path}/HASH").body.decode().strip()
        observed = request(
            method,
            path,
            b"replacement bytes",
            (
                ("If-Content-Match", content_hash),
                ("Authorization", f"Bearer {managed_token}"),
                ("Idempotency-Key", "replace-data-v2"),
            ),
        )
    elif example.request_line == "PATCH /hello/data.bin HTTP/1.1":
        content_hash = request("GET", f"{path}/HASH").body.decode().strip()
        observed = request(
            method,
            path,
            b"X",
            (
                ("If-Content-Match", content_hash),
                ("Splice", "offset=0; delete=0; insert=1"),
                ("Idempotency-Key", "splice-data-v1"),
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
        "ALIAS /hello/current HTTP/1.1": 201,
        "REPLACE /hello/data.bin HTTP/1.1": 200,
        "PATCH /hello/data.bin HTTP/1.1": 200,
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


def execute_phase_five_contract_examples() -> None:
    created = request("PUT", "/phase-five/index.txt", b"abcdef")
    require_status("Phase 5 fixture", created, 201)

    alias = request(
        "ALIAS",
        "/phase-five/current.txt",
        headers=(
            ("Alias-Target", "index.txt"),
            ("Idempotency-Key", "phase-five-alias"),
        ),
    )
    require_status("single alias", alias, 201)
    require_readable_location("single alias", alias)
    replayed_alias = request(
        "ALIAS",
        "/phase-five/current.txt",
        headers=(
            ("Alias-Target", "index.txt"),
            ("Idempotency-Key", "phase-five-alias"),
        ),
    )
    require_status("single alias replay", replayed_alias, 201)
    require_readable_location("single alias replay", replayed_alias)
    if replayed_alias.header("Idempotency-Replayed") != "true":
        raise AssertionError("single alias replay omitted Idempotency-Replayed")

    batch = request(
        "ALIAS",
        "/phase-five/",
        json.dumps(
            {
                "aliases": [
                    {"path": "one.txt", "target": "index.txt"},
                    {"path": "two.txt", "target": "current.txt"},
                ]
            },
            separators=(",", ":"),
        ).encode(),
        (
            ("Content-Type", "application/json"),
            ("Idempotency-Key", "phase-five-alias-batch"),
        ),
    )
    require_status("alias batch", batch, 201)
    require_readable_location("alias batch", batch)
    require_status("alias GET", request("GET", "/phase-five/two.txt"), 200)

    allocated = request(
        "POST",
        "/phase-five/generated/",
        b"allocated",
        (
            ("Content-Type", "application/octet-stream"),
            ("File-Prefix", "asset-"),
            ("File-Extension", ".BIN"),
            ("Idempotency-Key", "phase-five-allocate"),
        ),
    )
    require_status("allocated file", allocated, 201)
    require_readable_location("allocated file", allocated)
    allocated_body = json.loads(allocated.body)
    if allocated_body["naming"]["extension"] != "bin":
        raise AssertionError("allocated extension was not normalized")
    require_status(
        "allocated immutable content",
        request("GET", urllib.parse.urlsplit(allocated_body["blob_url"]).path),
        200,
    )

    original_hash = request("GET", "/phase-five/index.txt/HASH").body.decode().strip()
    replaced = request(
        "REPLACE",
        "/phase-five/index.txt",
        b"replacement",
        (
            ("If-Content-Match", original_hash),
            ("Idempotency-Key", "phase-five-replace"),
        ),
    )
    require_status("file replace", replaced, 200)
    require_readable_location("file replace", replaced)
    replacement_hash = json.loads(replaced.body)["new_hash"].removeprefix("blake3:")
    stale = request(
        "REPLACE",
        "/phase-five/index.txt",
        b"stale",
        (("If-Content-Match", original_hash),),
    )
    require_status("stale file replace", stale, 412)

    spliced = request(
        "PATCH",
        "/phase-five/index.txt",
        b"R",
        (
            ("If-Content-Match", replacement_hash),
            ("Splice", "offset=0; delete=1; insert=1"),
            ("Idempotency-Key", "phase-five-splice"),
        ),
    )
    require_status("header splice", spliced, 200)
    require_readable_location("header splice", spliced)
    spliced_hash = json.loads(spliced.body)["new_hash"].removeprefix("blake3:")

    frame = b"".join(
        (
            b"SYMSPL1\0",
            struct.pack(">IIQQQ", 1, 0, 0, 0, 1),
            b"F",
        )
    )
    framed = request(
        "PATCH",
        "/phase-five/index.txt",
        frame,
        (
            ("If-Content-Match", spliced_hash),
            ("Content-Type", "application/vnd.symbol.splice; version=1"),
            ("Idempotency-Key", "phase-five-frame"),
        ),
    )
    require_status("framed splice", framed, 200)
    require_readable_location("framed splice", framed)

    over_limit = request(
        "PATCH",
        "/phase-five/index.txt",
        headers=(
            (
                "If-Content-Match",
                json.loads(framed.body)["new_hash"].removeprefix("blake3:"),
            ),
            ("Splice", ",".join(["offset=0; delete=0; insert=0"] * 65)),
        ),
    )
    require_status("splice descriptor limit", over_limit, 413)

    proposed = request(
        "POST",
        "/phase-five/custom/",
        b"custom",
        (
            ("Allocation-Action", "propose"),
            ("Content-Type", "text/plain"),
            ("Idempotency-Key", "phase-five-propose"),
        ),
    )
    require_status("allocation proposal", proposed, 202)
    require_readable_location("allocation proposal", proposed)
    allocation_token = json.loads(proposed.body)["allocation_token"]
    finalized = request(
        "POST",
        "/phase-five/custom/",
        headers=(
            ("Allocation-Action", "finalize"),
            ("Allocation-Token", allocation_token),
            ("File-Name", "chosen"),
            ("Idempotency-Key", "phase-five-finalize"),
        ),
    )
    require_status("allocation finalization", finalized, 201)
    require_readable_location("allocation finalization", finalized)
    custom = request("GET", "/phase-five/custom/chosen")
    require_status("custom allocation media type", custom, 200)
    if custom.header("Content-Type") != "text/plain":
        raise AssertionError("custom allocation did not preserve its media type")

    inventory = request(
        "GET",
        "/phase-five/FILES",
        headers=(("Accept", "application/json"),),
    )
    require_status("alias inventory", inventory, 200)
    if len(json.loads(inventory.body)["aliases"]) != 3:
        raise AssertionError("JSON inventory omitted aliases")

    for target in (
        "/phase-five/FILES/generated/",
        "/phase-five/UNDO/x",
        "/phase-five/EXPIRES/x",
    ):
        rejected = request("POST", target, b"must not spool")
        require_status(f"virtual allocation rejection {target}", rejected, 400)
    inventory = request(
        "GET",
        "/phase-five/FILES",
        headers=(("Accept", "application/json"),),
    )
    payload = json.loads(inventory.body)
    mutation_paths = [entry["path"] for entry in payload["files"]]
    mutation_paths.extend(entry["path"] for entry in payload["aliases"])
    if any(
        path.split("/", 1)[0] in {"FILES", "UNDO", "EXPIRES"}
        for path in mutation_paths
    ):
        raise AssertionError("virtual namespace rejection left a stored mutation target")


claim = "sym_claim_" + "01" * 32
previous_hello = request("DELETE", "/hello")
if previous_hello.status not in (200, 404):
    raise AssertionError(
        "raw merge fixture reset: expected HTTP 200 or 404, "
        f"observed {previous_hello.status}"
    )
created = request(
    "PUT",
    "/hello/index.html",
    b"<h1>before raw merge</h1>\n",
    (("Management-Action", "claim"), ("Creator-Claim", claim)),
)
require_status("raw merge fixture", created, 201)
token = created.header("Management-Token")
inventory = request(
    "GET",
    "/hello/FILES",
    headers=(("Accept", "application/json"),),
)
require_status("raw merge fixture inventory", inventory, 200)
baseline = inventory.header("ETag")

examples = raw_http_examples()
for raw_example in examples:
    execute_raw_http(raw_example, token, baseline)

released_raw_state = request(
    "MANAGE",
    "/hello",
    headers=(
        ("Management-Action", "release"),
        ("Authorization", f"Bearer {token}"),
    ),
)
require_status("raw HTTP example release", released_raw_state, 200)

commands = curl_examples()
with tempfile.TemporaryDirectory() as work:
    environment = os.environ.copy()
    environment["SYMBOL_BASE"] = BASE
    pathlib.Path(work, "avatar.png").write_bytes(b"documented png fixture")
    for command in commands:
        subprocess.run(command, shell=True, check=True, cwd=work, env=environment)
    archive = pathlib.Path(work, "hello.tar.gz")
    with tarfile.open(archive, "r:gz") as packaged:
        if "symbol.toml" not in packaged.getnames():
            raise AssertionError("documented curl archive omitted symbol.toml")

execute_phase_five_contract_examples()
require_status("documented curl deletion", request("GET", "/hello/"), 404)
print(
    f"API examples: executed {len(examples)} raw HTTP requests and "
    f"{len(commands)} curl commands plus Phase 5 contract workflows"
)

#!/usr/bin/env python3
from __future__ import annotations

import dataclasses
import hashlib
import json
import os
import pathlib
import subprocess

EXPECTED_APPROVED_ADDITIONS_SHA256 = (
    "926704417789b34f482a46a7f53087740299ace41147c9a7139357dae51081c5"
)


@dataclasses.dataclass(frozen=True)
class SdkEntryPoints:
    javascript: str
    python: str


@dataclasses.dataclass(frozen=True)
class ApprovedAdditions:
    endpoint_names: tuple[str, ...]
    endpoint_contracts: tuple[Endpoint, ...]
    global_response_headers: tuple[str, ...]
    methods: tuple[str, ...]
    routes: tuple[str, ...]
    request_headers: tuple[str, ...]
    response_headers: tuple[str, ...]
    success_statuses: tuple[int, ...]
    error_statuses: tuple[int, ...]
    shell_commands: tuple[str, ...]
    sdk_assets: tuple[str, ...]
    sdk_entry_points: SdkEntryPoints
    allocated_name_formula: str
    splice_media_type: str
    alias_semantics: str


@dataclasses.dataclass(frozen=True)
class Freeze:
    format: int
    baseline_contract_sha256: str
    baseline_endpoint_sha256: tuple[BaselineEndpoint, ...]
    approved_additions: ApprovedAdditions


@dataclasses.dataclass(frozen=True)
class BaselineEndpoint:
    name: str
    sha256: str


@dataclasses.dataclass(frozen=True)
class Endpoint:
    name: str
    method: str
    head: bool
    path: str
    success_statuses: tuple[int, ...]
    error_statuses: tuple[int, ...]
    request_headers: tuple[str, ...]
    response_headers: tuple[str, ...]


def endpoint_from_json(value: dict[str, object]) -> Endpoint:
    return Endpoint(
        name=str(value["name"]),
        method=str(value["method"]),
        head=bool(value["head"]),
        path=str(value["path"]),
        success_statuses=tuple(value["success_statuses"]),
        error_statuses=tuple(value["error_statuses"]),
        request_headers=tuple(value["request_headers"]),
        response_headers=tuple(value["response_headers"]),
    )


def endpoint_hash(endpoint: Endpoint) -> str:
    payload = json.dumps(
        dataclasses.asdict(endpoint), sort_keys=True, separators=(",", ":")
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def validate_contract(freeze: Freeze, endpoints: tuple[Endpoint, ...]) -> None:
    assert len({endpoint.name for endpoint in endpoints}) == len(endpoints), (
        "contract endpoint names must be unique"
    )
    baseline_names = {endpoint.name for endpoint in freeze.baseline_endpoint_sha256}
    current_names = {endpoint.name for endpoint in endpoints}
    assert baseline_names <= current_names, (
        f"baseline endpoints removed: {sorted(baseline_names - current_names)}"
    )

    baseline_endpoints = tuple(
        endpoint for endpoint in endpoints if endpoint.name in baseline_names
    )
    for expected in freeze.baseline_endpoint_sha256:
        endpoint = next(
            endpoint for endpoint in baseline_endpoints if endpoint.name == expected.name
        )
        assert endpoint_hash(endpoint) == expected.sha256, (
            f"frozen endpoint changed: {expected.name}"
        )

    approved = freeze.approved_additions
    assert approved.endpoint_names == tuple(
        sorted(endpoint.name for endpoint in approved.endpoint_contracts)
    )
    for endpoint in endpoints:
        if endpoint.name in baseline_names:
            continue
        expected = next(
            (
                approved_endpoint
                for approved_endpoint in approved.endpoint_contracts
                if approved_endpoint.name == endpoint.name
            ),
            None,
        )
        assert expected is not None, f"unapproved endpoint name: {endpoint.name}"
        assert endpoint == expected, f"approved endpoint shape changed: {endpoint.name}"


root = pathlib.Path(__file__).resolve().parent.parent
symbol_bin = pathlib.Path(
    os.environ.get("SYMBOL_BIN", root / "target/debug/symbol")
)
raw = json.loads((root / "public-api-freeze.json").read_text())
additions_raw = raw["approved_additions"]
additions_payload = json.dumps(
    additions_raw, sort_keys=True, separators=(",", ":")
).encode()
assert hashlib.sha256(additions_payload).hexdigest() == (
    EXPECTED_APPROVED_ADDITIONS_SHA256
), "approved public additions changed without updating the freeze gate"
additions = ApprovedAdditions(
    endpoint_names=tuple(additions_raw["endpoint_names"]),
    endpoint_contracts=tuple(
        endpoint_from_json(endpoint)
        for endpoint in additions_raw["endpoint_contracts"]
    ),
    global_response_headers=tuple(additions_raw["global_response_headers"]),
    methods=tuple(additions_raw["methods"]),
    routes=tuple(additions_raw["routes"]),
    request_headers=tuple(additions_raw["request_headers"]),
    response_headers=tuple(additions_raw["response_headers"]),
    success_statuses=tuple(additions_raw["success_statuses"]),
    error_statuses=tuple(additions_raw["error_statuses"]),
    shell_commands=tuple(additions_raw["shell_commands"]),
    sdk_assets=tuple(additions_raw["sdk_assets"]),
    sdk_entry_points=SdkEntryPoints(**additions_raw["sdk_entry_points"]),
    allocated_name_formula=additions_raw["allocated_name_formula"],
    splice_media_type=additions_raw["splice_media_type"],
    alias_semantics=additions_raw["alias_semantics"],
)
freeze = Freeze(
    format=raw["format"],
    baseline_contract_sha256=raw["baseline_contract_sha256"],
    baseline_endpoint_sha256=tuple(
        BaselineEndpoint(name=name, sha256=sha256)
        for name, sha256 in raw["baseline_endpoint_sha256"].items()
    ),
    approved_additions=additions,
)
assert freeze.format == 1

contract_raw = json.loads(
    subprocess.check_output([symbol_bin, "contract"], text=True)
)
contract = tuple(endpoint_from_json(endpoint) for endpoint in contract_raw)
canonical = json.dumps(contract_raw, sort_keys=True, separators=(",", ":")).encode()
observed = hashlib.sha256(canonical).hexdigest()
validate_contract(freeze, contract)
if len(contract) == len(freeze.baseline_endpoint_sha256):
    assert observed == freeze.baseline_contract_sha256

for field in dataclasses.fields(ApprovedAdditions):
    value = getattr(freeze.approved_additions, field.name)
    if isinstance(value, tuple):
        if field.name == "endpoint_contracts":
            assert tuple(endpoint.name for endpoint in value) == tuple(
                sorted(endpoint.name for endpoint in value)
            ), "endpoint_contracts must be sorted by name"
            continue
        assert value == tuple(sorted(set(value))), (
            f"{field.name} must be sorted and unique"
        )

changed_baseline = dataclasses.replace(contract[0], path="/unapproved-change")
try:
    validate_contract(freeze, (changed_baseline, *contract[1:]))
except AssertionError:
    pass
else:
    raise AssertionError("changed baseline endpoint was accepted")

approved_extra = next(
    endpoint
    for endpoint in freeze.approved_additions.endpoint_contracts
    if endpoint.name == "api version"
)
contract_without_extra = tuple(
    endpoint for endpoint in contract if endpoint.name != approved_extra.name
)
validate_contract(freeze, (*contract_without_extra, approved_extra))
try:
    validate_contract(
        freeze,
        (
            *contract,
            dataclasses.replace(
                approved_extra,
                name="unapproved endpoint",
                path="/unapproved",
            ),
        ),
    )
except AssertionError:
    pass
else:
    raise AssertionError("unapproved added endpoint was accepted")

print(
    f"public API freeze: {len(freeze.baseline_endpoint_sha256)} baseline endpoints, "
    f"{len(freeze.approved_additions.endpoint_names)} approved additions"
)

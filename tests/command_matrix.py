#!/usr/bin/env python3
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
client = root / "static/symbol.sh"
registry_environment = os.environ | {"SYMBOL_TEST_COMMAND_REGISTRY": "1"}
registry_output = subprocess.check_output(
    [client], env=registry_environment, text=True
)

spellings = {}
for line in registry_output.splitlines():
    canonical, *names = line.split()
    for name in names:
        spellings[name] = canonical

tokens = set(spellings)
for spelling in spellings:
    for start in range(len(spelling)):
        for end in range(start + 1, len(spelling) + 1):
            tokens.add(spelling[start:end])


def expected(token):
    if token in spellings:
        return {spellings[token]}
    prefixes = {
        canonical
        for spelling, canonical in spellings.items()
        if spelling.startswith(token)
    }
    if prefixes:
        return prefixes
    return {
        canonical
        for spelling, canonical in spellings.items()
        if token in spelling
    }


resolve_environment = os.environ | {"SYMBOL_TEST_RESOLVE_ONLY": "1"}
failures = []
for token in sorted(tokens):
    identities = expected(token)
    result = subprocess.run(
        [client, token],
        env=resolve_environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if len(identities) == 1:
        wanted = next(iter(identities))
        if result.returncode != 0 or result.stdout.strip() != wanted:
            failures.append(
                f"{token!r}: expected {wanted!r}, got "
                f"exit={result.returncode} stdout={result.stdout.strip()!r}"
            )
    elif result.returncode != 2 or "ambiguous command" not in result.stderr:
        failures.append(
            f"{token!r}: expected ambiguity among {sorted(identities)}, got "
            f"exit={result.returncode} stderr={result.stderr.strip()!r}"
        )

if failures:
    print("\n".join(failures), file=sys.stderr)
    sys.exit(1)

print(
    f"command matrix: {len(spellings)} exact spellings, "
    f"{len(tokens)} exact/prefix/substring cases"
)

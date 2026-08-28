#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
API = ROOT / "API.md"


def generated_dir() -> Path:
    configured = os.environ.get("SYMBOL_GENERATED_DIR")
    if configured:
        return Path(configured)
    candidates = sorted(
        (ROOT / "target" / "debug" / "build").glob("symbol-*/out/symbol.py"),
        key=lambda path: path.stat().st_mtime_ns,
        reverse=True,
    )
    if not candidates:
        raise RuntimeError("generated SDK artifacts are unavailable")
    return candidates[0].parent


def marked_example(source: str, language: str) -> str:
    start = f"<!-- EXEC:{language}:START -->"
    end = f"<!-- EXEC:{language}:END -->"
    if source.count(start) != 1 or source.count(end) != 1:
        raise AssertionError(f"{language} needs exactly one executable example")
    body = source.split(start, 1)[1].split(end, 1)[0].strip()
    opening, code, closing = body.split("\n", 2)
    fence = {"TS": "ts", "PYTHON": "python", "SHELL": "sh"}[language]
    if opening != f"```{fence}" or not closing.endswith("\n```"):
        raise AssertionError(f"{language} executable example has malformed fencing")
    return code + "\n" + closing.removesuffix("\n```") + "\n"


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def wait_ready(base: str, process: subprocess.Popen[bytes]) -> None:
    for _ in range(100):
        if process.poll() is not None:
            raise RuntimeError(f"temporary Symbol exited with {process.returncode}")
        try:
            with urllib.request.urlopen(f"{base}/STATS", timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("temporary Symbol did not become ready")


def run() -> None:
    source = API.read_text()
    examples = {
        language: marked_example(source, language)
        for language in ("TS", "PYTHON", "SHELL")
    }
    artifacts = generated_dir()
    symbol_bin = Path(os.environ.get("SYMBOL_BIN", ROOT / "target/debug/symbol"))
    tsc = os.environ.get("TSC", "tsc")

    with tempfile.TemporaryDirectory(prefix="symbol-manual-examples-") as raw:
        work = Path(raw)
        data = work / "data"
        data.mkdir()
        port = free_port()
        base = f"http://127.0.0.1:{port}"
        server = subprocess.Popen(
            [
                str(symbol_bin),
                "--bind",
                f"127.0.0.1:{port}",
                "--root",
                str(data),
                "--public-url",
                base,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        try:
            wait_ready(base, server)
            run_typescript(work, artifacts, examples["TS"], base, tsc)
            run_python(work, artifacts, examples["PYTHON"], base)
            run_shell(work, examples["SHELL"], base)
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
    print("manual examples: TypeScript, Python, and shell workflows passed")


def run_typescript(
    work: Path,
    artifacts: Path,
    example: str,
    base: str,
    tsc: str,
) -> None:
    directory = work / "typescript"
    output = directory / "out"
    directory.mkdir()
    (directory / "workflow.ts").write_text(example)
    shutil.copy2(artifacts / "symbol.d.ts", directory / "symbol.d.ts")
    subprocess.run(
        [
            tsc,
            "workflow.ts",
            "--strict",
            "--target",
            "ES2022",
            "--module",
            "ES2022",
            "--moduleResolution",
            "bundler",
            "--lib",
            "ES2022,DOM,ESNext.Disposable",
            "--outDir",
            str(output),
        ],
        cwd=directory,
        check=True,
    )
    shutil.copy2(artifacts / "symbol.js", output / "symbol.js")
    (output / "package.json").write_text('{"type":"module"}\n')
    module = (output / "workflow.js").as_uri()
    url = f"{module}?origin={urllib.parse.quote(base, safe='')}"
    subprocess.run(
        ["node", "--eval", f"await import({json.dumps(url)})"],
        cwd=directory,
        check=True,
    )


def run_python(work: Path, artifacts: Path, example: str, base: str) -> None:
    directory = work / "python"
    directory.mkdir()
    (directory / "workflow.py").write_text(example)
    shutil.copy2(artifacts / "symbol.py", directory / "symbol_api.py")
    subprocess.run(
        [sys.executable, "workflow.py"],
        cwd=directory,
        env={**os.environ, "SYMBOL_BASE": base},
        check=True,
    )


def run_shell(work: Path, example: str, base: str) -> None:
    directory = work / "shell"
    binary = directory / "bin"
    distribution = directory / "dist"
    (distribution / "assets").mkdir(parents=True)
    binary.mkdir()
    shutil.copy2(ROOT / "static" / "symbol.sh", binary / "symbol")
    (binary / "symbol").chmod(0o755)
    (distribution / "index.html").write_text("<h1>Manual example</h1>\n")
    (distribution / "assets" / "demo.mp4").write_bytes(b"demo")
    (directory / "robots.txt").write_text("User-agent: *\nDisallow:\n")
    (directory / "workflow.sh").write_text(example)
    subprocess.run(
        ["sh", "workflow.sh"],
        cwd=directory,
        env={
            **os.environ,
            "PATH": f"{binary}{os.pathsep}{os.environ['PATH']}",
            "SYMBOL_HOST": base,
            "XDG_STATE_HOME": str(directory / "state"),
        },
        check=True,
    )


if __name__ == "__main__":
    run()

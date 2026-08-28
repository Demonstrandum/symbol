#!/usr/bin/env python3
from __future__ import annotations

import asyncio
import importlib.util
import io
import json
import os
import pathlib
import sys
from collections.abc import AsyncIterator
from dataclasses import dataclass

ROOT = pathlib.Path(__file__).resolve().parents[3]


def generated_api() -> pathlib.Path:
    generated_dir = os.environ.get("SYMBOL_GENERATED_DIR")
    if generated_dir:
        path = pathlib.Path(generated_dir) / "symbol.py"
        if path.is_file():
            return path
    candidates = sorted(
        ROOT.glob("target/debug/build/symbol-*/out/symbol.py"),
        key=lambda path: path.stat().st_mtime_ns,
    )
    if not candidates:
        raise AssertionError("generated symbol.py is missing; build symbol first")
    return candidates[-1]


def load_api():
    spec = importlib.util.spec_from_file_location("symbol_api", generated_api())
    if spec is None or spec.loader is None:
        raise AssertionError("cannot load generated symbol.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


api = load_api()
IDENTITY = (
    ("Symbol-API-Version", api.API_VERSION),
    ("Symbol-API-Revision", str(api.API_REVISION)),
    ("Symbol-API-Source-Hash", api.SOURCE_HASH),
)


def stats_payload(*, sites: int = 0, files: object = 0) -> dict[str, object]:
    distribution = {
        "min": None,
        "p25": None,
        "median": None,
        "mean": None,
        "p75": None,
        "max": None,
        "iqr": None,
        "stddev": None,
    }
    return {
        "sites": sites,
        "files": files,
        "aliases": 0,
        "blobs": 0,
        "bytes": 0,
        "logical_bytes": 0,
        "saved_bytes": 0,
        "saved_fraction": 0.0,
        "file_sizes": distribution,
        "blob_sizes": distribution,
        "serving": {
            "cache": {"hits": 0, "misses": 0, "evictions": 0},
            "readers": {
                "operations": 0,
                "waits": 0,
                "wait_micros": 0,
                "query_micros": 0,
            },
        },
    }


@dataclass
class Recorded:
    method: str
    url: str
    headers: tuple[tuple[str, str], ...]
    body: bytes | None


class Backend:
    def __init__(self) -> None:
        self.requests: list[Recorded] = []
        self.responses: list[api.ApiResponse] = []
        self.closed = 0

    def queue(
        self,
        status: int,
        body: object = b"",
        headers: tuple[tuple[str, str], ...] = (),
    ) -> None:
        encoded = (
            json.dumps(body, separators=(",", ":")).encode()
            if not isinstance(body, bytes)
            else body
        )
        self.responses.append(api.ApiResponse(status, IDENTITY + headers, encoded))

    def request(self, request: api.ApiRequest) -> api.ApiResponse:
        body = api._body(request.body)
        self.requests.append(
            Recorded(str(request.method), request.url, request.headers, body)
        )
        if not self.responses:
            raise AssertionError(f"unexpected request {request.method} {request.url}")
        return self.responses.pop(0)

    def close(self) -> None:
        self.closed += 1


class AsyncBackend:
    def __init__(self, response: api.ApiResponse) -> None:
        self.response = response
        self.requests: list[api.ApiRequest] = []
        self.closed = 0

    async def request(self, request: api.AsyncApiRequest) -> api.ApiResponse:
        self.requests.append(request)
        return self.response

    async def close(self) -> None:
        self.closed += 1


class FlakyBackend(Backend):
    def __init__(self, failures: int) -> None:
        super().__init__()
        self.failures = failures

    def request(self, request: api.ApiRequest) -> api.ApiResponse:
        if self.failures:
            self.failures -= 1
            raise OSError("network unavailable")
        return super().request(request)


class FlakyAsyncBackend(AsyncBackend):
    def __init__(self, response: api.ApiResponse, failures: int) -> None:
        super().__init__(response)
        self.failures = failures

    async def request(self, request: api.AsyncApiRequest) -> api.ApiResponse:
        if self.failures:
            self.failures -= 1
            raise OSError("network unavailable")
        return await super().request(request)


def mutation_headers(path: str = "x.txt") -> tuple[tuple[str, str], ...]:
    return (
        ("Location", f"http://symbol/demo/{path}"),
        ("ETag", '"blake3:' + "c" * 64 + '"'),
        ("Content-Revision", "2"),
        ("Undo-Token", "a" * 32),
        ("Undo-Expires", "2026-08-27T00:00:00Z"),
    )


def test_metadata_and_media_types() -> None:
    assert api.METADATA.artifact is api.ApiArtifact.PYTHON
    assert api.API_VERSION.count(".") == 2
    assert str(api.API_VERSION_PARTS) == api.API_VERSION
    assert api.ApiVersion.parse(api.API_VERSION) == api.API_VERSION_PARTS
    assert isinstance(api.SOURCE_HASH, api.Blake3)
    assert api.API_REVISION > 0
    assert len(api.SOURCE_HASH) == 64
    assert str(api.MediaTypes.JSON) == "application/json"
    assert str(api.MediaType.text("plain")) == "text/plain; charset=utf-8"
    assert api.ContentFormats.HTML.extensions == ("html", "htm")
    assert api.MediaType.parse('text/plain; charset="utf-8"') == api.MediaTypes.TEXT
    for invalid in ("1.02.3", "1.2", "1.2.x"):
        try:
            api.ApiVersion.parse(invalid)
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted invalid semantic version {invalid!r}")


def test_sync_client_mapping() -> None:
    backend = Backend()
    client = api.HttpClient.wrap(backend)
    symbol = api.Symbol(client, origin="http://symbol")

    backend.queue(
        200,
        stats_payload(sites=1, files=2),
        (("Content-Type", "application/json"),),
    )
    stats = symbol.stats()
    assert stats.files == 2
    assert isinstance(stats.file_sizes, api.SizeDistribution)
    assert isinstance(stats.serving.cache, api.CacheStats)

    backend.queue(200, b"export {}", (("Content-Type", "text/typescript"),))
    assert symbol.api_client(api.ApiClientAsset.TYPESCRIPT).status == 200
    assert backend.requests[-1].url == "http://symbol/symbol.ts"

    backend.queue(200, ("a" * 64 + "\n").encode())
    assert symbol.api_client_hash(api.ApiClientAsset.TYPESCRIPT) == "a" * 64

    backend.queue(200, b"# TypeScript", (("Content-Type", "text/markdown"),))
    assert symbol.api_manual(api.ApiManual.TYPESCRIPT).text() == "# TypeScript"

    backend.queue(
        200,
        {
            "api_version": api.API_VERSION,
            "absolute_revision": api.API_REVISION,
            "source_hash": api.SOURCE_HASH,
        },
        (("Content-Type", "application/json"),),
    )
    assert symbol.api_version().source_hash == api.SOURCE_HASH

    backend.queue(200, b"hello", (("Content-Type", "text/plain"),))
    assert symbol.site("demo").file("space name.txt").text() == "hello"
    assert backend.requests[-1].url.endswith("/demo/space%20name.txt")

    backend.queue(200, b"ok (changed: true)", mutation_headers())
    receipt = symbol.site("demo").file("x.txt").put("value", api.MediaTypes.TEXT)
    assert receipt.changed and receipt.undo is not None
    assert not any(
        name == "Idempotency-Key" for name, _ in backend.requests[-1].headers
    )

    backend.queue(
        201,
        {
            "changed": True,
            "path": "note/hash.anno",
            "target": "target",
            "target_kind": "file",
            "dangling": False,
        },
        (("Content-Type", "application/json"), *mutation_headers("note/hash.anno")),
    )
    symbol.site("demo").alias("note/hash.anno", "target")
    request = backend.requests[-1]
    assert request.method == "ALIAS"
    assert any(name == "Idempotency-Key" for name, _ in request.headers)

    client.close()
    assert backend.closed == 0  # wrapped backends are borrowed

    context_backend = Backend()
    context_backend.queue(
        200,
        stats_payload(),
        (("Content-Type", "application/json"),),
    )
    with api.Symbol(api.HttpClient.wrap(context_backend)) as context_symbol:
        assert context_symbol.stats().sites == 0
    assert context_backend.closed == 0


def test_allocation_naming() -> None:
    backend = Backend()
    symbol = api.Symbol(api.HttpClient.wrap(backend), origin="http://symbol")
    backend.queue(
        201,
        {
            "changed": True,
            "site": "demo",
            "path": "notes/pre-hash-final.anno",
            "hash": "blake3:" + "b" * 64,
            "blob_url": "/.blob/demo/" + "b" * 64,
        },
        (
            ("Content-Type", "application/json"),
            *mutation_headers("notes/pre-hash-final.anno"),
        ),
    )
    options = api.CreateFileOptions(
        name=api.GeneratedName(prefix="pre-", suffix="-final", extension=".anno"),
        media_type=api.MediaTypes.BINARY,
    )
    symbol.site("demo").folder("notes").bytes(b"opaque", options)
    headers = dict(backend.requests[-1].headers)
    assert headers["File-Prefix"] == "pre-"
    assert headers["File-Suffix"] == "-final"
    assert headers["File-Extension"] == "anno"


def test_sync_async_public_method_parity() -> None:
    def methods(client_type: type[object]) -> set[str]:
        return {
            name
            for name in dir(client_type)
            if not name.startswith("_") and callable(getattr(client_type, name))
        }

    for sync_type, async_type in (
        (api._SymbolSync, api._SymbolAsync),
        (api.SiteClient, api.AsyncSiteClient),
        (api.FolderClient, api.AsyncFolderClient),
        (api.FileClient, api.AsyncFileClient),
        (api.ManagementClient, api.AsyncManagementClient),
    ):
        assert methods(sync_type) == methods(async_type), (
            sync_type.__name__,
            methods(sync_type) ^ methods(async_type),
        )


def test_malformed_responses_and_typed_errors() -> None:
    backend = Backend()
    symbol = api.Symbol(api.HttpClient.wrap(backend), origin="http://symbol")

    backend.queue(
        200,
        stats_payload(files="wrong"),
        (("Content-Type", "application/json"),),
    )
    try:
        symbol.stats()
    except api.MalformedResponseError:
        pass
    else:
        raise AssertionError("malformed stats response was accepted")

    backend.queue(
        412,
        b"stale",
        (("ETag", '"blake3:current"'), ("Content-Revision", "7")),
    )
    try:
        symbol.site("demo").file("x").put(b"x")
    except api.PreconditionFailedError as error:
        assert error.etag == '"blake3:current"'
        assert error.content_revision == 7
    else:
        raise AssertionError("precondition failure was not typed")

    backend.queue(
        401,
        b"token required",
        (("WWW-Authenticate", 'Bearer realm="symbol"'),),
    )
    try:
        symbol.site("demo").file("x").put(b"x")
    except api.UnauthorizedError as error:
        assert error.challenge == 'Bearer realm="symbol"'
    else:
        raise AssertionError("unauthorized response was not typed")

    backend.queue(
        416,
        b"range",
        (("Content-Range", "bytes */10"),),
    )
    try:
        symbol.site("demo").file("x").replace(b"x", base_hash="a" * 64)
    except api.RangeNotSatisfiableError as error:
        assert error.content_range == "bytes */10"
    else:
        raise AssertionError("range failure was not typed")

    claim_backend = Backend()
    claim_symbol = api.Symbol(
        api.HttpClient.wrap(claim_backend),
        origin="http://symbol",
        creator_claim="sym_claim_" + "1" * 64,
    )
    claim_backend.queue(
        200,
        {"managed": True},
        (
            ("Content-Type", "application/json"),
            ("Management-Token", "sym_mgmt_" + "2" * 64),
        ),
    )
    claim_symbol.site("demo").management().claim()
    assert dict(claim_backend.requests[-1].headers)["Creator-Claim"] == (
        "sym_claim_" + "1" * 64
    )

    expiry_backend = Backend()
    expiry_symbol = api.Symbol(api.HttpClient.wrap(expiry_backend))
    expiry_backend.queue(
        200,
        {
            "target": {"site": "demo", "path": "x", "kind": "file"},
            "size": 10,
            "refreshed_at": None,
            "own_policy": {
                "mode": "decay",
                "min_age_seconds": 60,
                "max_age_seconds": 3600,
                "max_size_bytes": 100,
                "power": 2.0,
                "retention_seconds": 600,
                "expires_at": "2026-08-27T12:00:00Z",
            },
            "inherited_caps": [],
            "effective_expires_at": "2026-08-27T12:00:00Z",
            "remaining_seconds": 600,
            "limited_by": {"kind": "site", "path": None},
        },
        (("Content-Type", "application/json"),),
    )
    report = expiry_symbol.site("demo").expiry("x")
    assert isinstance(report, api.ExpiryReport)
    assert isinstance(report.own_policy, api.DecayExpiryPolicy)
    assert isinstance(report.limited_by, api.ExpiryLimit)


def test_retry_policy_applies_to_sync_and_async_transports() -> None:
    class ProgrammingErrorBackend:
        def request(self, request: api.ApiRequest) -> api.ApiResponse:
            raise ValueError("programming error")

        def close(self) -> None:
            pass

    try:
        api.HttpClient.wrap(ProgrammingErrorBackend()).request(
            api.ApiRequest(api.HttpMethod.GET, "http://symbol/STATS")
        )
    except ValueError as error:
        assert str(error) == "programming error"
    else:
        raise AssertionError(
            "backend programming error was normalized as network failure"
        )

    response = api.ApiResponse(
        200,
        IDENTITY + (("Content-Type", "application/json"),),
        json.dumps(stats_payload(), separators=(",", ":")).encode(),
    )
    sync_backend = FlakyBackend(1)
    sync_backend.responses.append(response)
    policy = api.RetryPolicy(
        max_attempts=2,
        initial_delay=0,
        maximum_delay=0,
        retry_network_errors=True,
    )
    retry_after = api.ApiResponse(
        503,
        IDENTITY + (("Retry-After", "999"),),
        b"retry",
    )
    assert (
        api._retry_delay(
            api.RetryPolicy(maximum_delay=2.0),
            0,
            retry_after,
        )
        == 2.0
    )
    assert (
        api.Symbol(api.HttpClient.wrap(sync_backend), retry_policy=policy).stats().sites
        == 0
    )

    successful_backend = Backend()
    successful_backend.queue(200, b"ok")
    successful = api.Symbol(api.HttpClient.wrap(successful_backend)).request(
        api.ApiRequest(api.HttpMethod.GET, "/success")
    )
    try:
        successful.retry()
    except api.OperationStateError:
        pass
    else:
        raise AssertionError("successful response was manually retryable")

    terminal_backend = FlakyBackend(1)
    terminal_backend.responses.append(response)
    terminal_symbol = api.Symbol(
        api.HttpClient.wrap(terminal_backend),
        retry_policy=api.RetryPolicy(
            max_attempts=1,
            initial_delay=0,
            maximum_delay=0,
            retry_network_errors=True,
        ),
    )
    try:
        terminal_symbol.stats()
    except api.NetworkRequestError as error:
        assert error.replayable
        assert len(error.attempts) == 1
        retried = error.retry(api.RetryPolicy(max_attempts=1))
        assert isinstance(retried, api.SymbolStats)
        assert retried.sites == 0
        try:
            error.retry()
        except api.OperationStateError:
            pass
        else:
            raise AssertionError("successful typed retry remained reusable")
    else:
        raise AssertionError("terminal network failure did not fail")

    manual_backend = Backend()
    manual_backend.queue(503, b"retry")
    manual_backend.responses.append(response)
    manual_symbol = api.Symbol(api.HttpClient.wrap(manual_backend))
    try:
        manual_symbol.stats()
    except api.ServerError as error:
        assert len(error.attempts) == 1
        retried = error.retry(api.RetryPolicy(max_attempts=1))
        assert isinstance(retried, api.SymbolStats)
        assert retried.sites == 0
    else:
        raise AssertionError("manual retry fixture did not fail")

    allocation_backend = Backend()
    allocation_backend.queue(503, b"retry")
    allocation_backend.queue(
        201,
        {
            "changed": True,
            "site": "demo",
            "path": "notes/generated.bin",
            "hash": "blake3:" + "b" * 64,
            "blob_url": "/.blob/demo/" + "b" * 64,
        },
        (
            ("Content-Type", "application/json"),
            *mutation_headers("notes/generated.bin"),
        ),
    )
    allocation_symbol = api.Symbol(api.HttpClient.wrap(allocation_backend))
    try:
        allocation_symbol.site("demo").folder("notes").bytes(b"value")
    except api.ServerError as error:
        retried = error.retry(api.RetryPolicy(max_attempts=1))
        assert isinstance(retried, api.AllocationReceipt)
        assert retried.path == "notes/generated.bin"
    else:
        raise AssertionError("allocation retry fixture did not fail")

    mutation_backend = Backend()
    mutation_backend.queue(503, b"retry")
    mutation_backend.queue(
        200,
        {"changed": True},
        (("Content-Type", "application/json"), *mutation_headers()),
    )
    mutation_symbol = api.Symbol(api.HttpClient.wrap(mutation_backend))
    try:
        mutation_symbol.site("demo").file("x.txt").replace(
            b"next",
            base_hash=api.Blake3("c" * 64),
        )
    except api.ServerError as error:
        retried = error.retry(api.RetryPolicy(max_attempts=1))
        assert isinstance(retried, api.MutationReceipt)
        assert retried.changed
    else:
        raise AssertionError("mutation retry fixture did not fail")

    non_replayable_backend = Backend()
    non_replayable_backend.queue(503, b"retry")
    non_replayable_symbol = api.Symbol(
        api.HttpClient.wrap(non_replayable_backend),
        retry_policy=api.RetryPolicies.DEFAULT,
    )
    try:
        non_replayable_symbol.site("demo").file("x").put(io.BytesIO(b"once"))
    except api.ServerError as error:
        assert not error.replayable
        try:
            error.retry()
        except api.BodyNotReplayableError:
            pass
        else:
            raise AssertionError("non-replayable request allowed manual retry")
    else:
        raise AssertionError("non-replayable retry fixture did not fail")

    async def check_async() -> None:
        async_backend = FlakyAsyncBackend(response, 1)
        symbol = api.Symbol(
            api.AsyncHttpClient.wrap(async_backend),
            retry_policy=policy,
        )
        assert (await symbol.stats()).sites == 0

        successful_backend = AsyncBackend(api.ApiResponse(200, IDENTITY, b"ok"))
        successful_symbol = api.Symbol(api.AsyncHttpClient.wrap(successful_backend))
        successful = await successful_symbol.request(
            api.AsyncApiRequest(api.HttpMethod.GET, "/success")
        )
        try:
            await successful.aretry()
        except api.OperationStateError:
            pass
        else:
            raise AssertionError("successful async response was manually retryable")

        terminal_backend = FlakyAsyncBackend(response, 1)
        terminal_symbol = api.Symbol(
            api.AsyncHttpClient.wrap(terminal_backend),
            retry_policy=api.RetryPolicy(
                max_attempts=1,
                initial_delay=0,
                maximum_delay=0,
                retry_network_errors=True,
            ),
        )
        try:
            await terminal_symbol.stats()
        except api.NetworkRequestError as error:
            retried = await error.aretry(api.RetryPolicy(max_attempts=1))
            assert isinstance(retried, api.SymbolStats)
            assert retried.sites == 0
            try:
                await error.aretry()
            except api.OperationStateError:
                pass
            else:
                raise AssertionError("successful async typed retry remained reusable")
        else:
            raise AssertionError("async terminal network failure did not fail")

        manual_backend = AsyncBackend(api.ApiResponse(503, IDENTITY, b"retry"))
        manual_symbol = api.Symbol(api.AsyncHttpClient.wrap(manual_backend))
        try:
            await manual_symbol.stats()
        except api.ServerError as error:
            manual_backend.response = response
            retried = await error.aretry(api.RetryPolicy(max_attempts=1))
            assert isinstance(retried, api.SymbolStats)
            assert retried.sites == 0
        else:
            raise AssertionError("async manual retry fixture did not fail")

    asyncio.run(check_async())


def test_streaming_bodies_are_lazy_and_closeable() -> None:
    try:
        api._async_body(pathlib.Path("/sync-path-is-not-an-async-source"))
    except TypeError:
        pass
    else:
        raise AssertionError("async transport accepted a synchronous Path source")

    reads = 0
    closes = 0

    class StreamingBackend:
        def request(self, request: api.ApiRequest) -> api.ApiResponse:
            nonlocal reads, closes
            assert isinstance(request.body, pathlib.Path)
            source = io.BytesIO(b"abcdef")

            def read(size: int) -> bytes:
                nonlocal reads
                reads += 1
                return source.read(size)

            def iterate(chunk_size: int):
                while chunk := read(chunk_size):
                    yield chunk

            def close() -> None:
                nonlocal closes
                closes += 1
                source.close()

            return api.ApiResponse(
                200,
                IDENTITY + (("Content-Type", "application/octet-stream"),),
                stream=api.SyncResponseStream(read, iterate, close),
            )

        def close(self) -> None:
            pass

    response = api.Symbol(api.HttpClient.wrap(StreamingBackend())).request(
        api.ApiRequest(api.HttpMethod.PUT, "/stream", body=pathlib.Path("/unused"))
    )
    assert reads == 0
    assert b"".join(response.iter_bytes(2)) == b"abcdef"
    assert reads == 4
    assert closes == 1
    response.close()
    assert closes == 1

    async def check_async() -> None:
        async_reads = 0
        async_closes = 0

        class StreamingAsyncBackend:
            async def request(self, request: api.AsyncApiRequest) -> api.ApiResponse:
                assert hasattr(request.body, "__aiter__")
                source = io.BytesIO(b"ghijkl")

                async def read() -> bytes:
                    nonlocal async_reads
                    async_reads += 1
                    return source.read()

                async def iterate(chunk_size: int):
                    nonlocal async_reads
                    while chunk := source.read(chunk_size):
                        async_reads += 1
                        yield chunk

                async def close() -> None:
                    nonlocal async_closes
                    async_closes += 1
                    source.close()

                return api.ApiResponse(
                    200,
                    IDENTITY + (("Content-Type", "application/octet-stream"),),
                    async_stream=api.AsyncResponseStream(read, iterate, close),
                )

            async def close(self) -> None:
                pass

        symbol = api.Symbol(
            api.AsyncHttpClient.wrap(StreamingAsyncBackend()),
            origin="http://symbol",
        )

        async def upload() -> AsyncIterator[bytes]:
            yield b"upload"

        response = await symbol.request(
            api.AsyncApiRequest(
                api.HttpMethod.GET,
                "/stream",
                body=upload(),
            )
        )
        assert async_reads == 0
        observed = bytearray()
        async for chunk in response.aiter_bytes(2):
            observed.extend(chunk)
        assert observed == b"ghijkl"
        assert async_reads == 3
        assert async_closes == 1

    asyncio.run(check_async())


def test_async_splice_insertions_never_read_synchronous_sources() -> None:
    async def chunks():
        yield b"ab"
        yield b"cd"

    async def check() -> None:
        assert await api._async_splice_insert(b"bytes") == b"bytes"
        assert await api._async_splice_insert(memoryview(b"view")) == b"view"
        assert await api._async_splice_insert("text") == b"text"
        assert await api._async_splice_insert(chunks()) == b"abcd"
        for source in (
            pathlib.Path("/must-not-be-read"),
            io.BytesIO(b"must-not-be-read"),
        ):
            try:
                await api._async_splice_insert(source)
            except TypeError:
                pass
            else:
                raise AssertionError("async splice accepted a synchronous source")

    assert "AsyncByteSplice" in api.__dict__
    asyncio.run(check())


async def test_async_factory() -> None:
    response = api.ApiResponse(
        200,
        IDENTITY + (("Content-Type", "application/json"),),
        json.dumps(stats_payload(), separators=(",", ":")).encode(),
    )
    backend = AsyncBackend(response)
    client = api.AsyncHttpClient.wrap(backend)
    symbol = api.Symbol(client, origin="http://symbol")
    async with symbol:
        assert (await symbol.stats()).sites == 0
        backend.response = api.ApiResponse(
            200,
            IDENTITY + (("Content-Type", "text/plain"),),
            b"async",
        )
        assert await symbol.site("demo").file("x.txt").text() == "async"
    assert backend.closed == 0


def main() -> None:
    test_metadata_and_media_types()
    test_sync_client_mapping()
    test_allocation_naming()
    test_sync_async_public_method_parity()
    test_malformed_responses_and_typed_errors()
    test_retry_policy_applies_to_sync_and_async_transports()
    test_streaming_bodies_are_lazy_and_closeable()
    test_async_splice_insertions_never_read_synchronous_sources()
    asyncio.run(test_async_factory())
    print("SDK Python runtime: 9 tests")


if __name__ == "__main__":
    main()

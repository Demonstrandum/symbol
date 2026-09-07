from __future__ import annotations

import asyncio
import email.utils
import http.client
import inspect
import json as _json
import math
import secrets
import time
from collections.abc import (
    AsyncIterable,
    AsyncIterator,
    Awaitable,
    Callable,
    Iterable,
    Iterator,
    Mapping,
)
from dataclasses import dataclass, replace
from datetime import UTC, datetime
from enum import IntEnum, StrEnum
from pathlib import Path
from types import TracebackType
from typing import (
    TYPE_CHECKING,
    Any,
    Final,
    Protocol,
    Self,
    cast,
    overload,
)
from urllib.parse import quote, urlsplit

if TYPE_CHECKING:
    import aiohttp
    import httpx
    import requests
    import urllib3


type IdempotencyKey = str
type AllocationToken = str
type SiteName = str
type SitePath = str
type ResourceUrl = str


@dataclass(frozen=True, order=True, slots=True)
class ApiVersion:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, value: str) -> Self:
        match value.split("."):
            case [major, minor, patch] if all(
                part == "0"
                or (part.isascii() and part.isdecimal() and not part.startswith("0"))
                for part in (major, minor, patch)
            ):
                return cls(int(major), int(minor), int(patch))
            case _:
                raise ValueError(f"invalid semantic API version: {value!r}")

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"


class Blake3(str):
    def __new__(cls, value: str) -> Self:
        if len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
            raise ValueError(f"invalid Blake3 hash: {value!r}")
        return str.__new__(cls, value)


class GitCommit(str):
    def __new__(cls, value: str) -> Self:
        if value == "unknown":
            return str.__new__(cls, value)
        if not 7 <= len(value) <= 64 or any(
            char not in "0123456789abcdefABCDEF" for char in value
        ):
            raise ValueError(f"invalid Git commit: {value!r}")
        return str.__new__(cls, value)


class ApiArtifact(StrEnum):
    TYPESCRIPT = "symbol.ts"
    JAVASCRIPT = "symbol.js"
    GLOBAL_JAVASCRIPT = "symbol.global.js"
    DECLARATIONS = "symbol.d.ts"
    PYTHON = "symbol.py"


class TreeHash(str):
    def __new__(cls, value: str) -> Self:
        prefix, separator, payload = value.partition(":")
        if prefix != "blake3" or separator != ":":
            raise ValueError(f"invalid tree hash: {value!r}")
        Blake3(payload)
        return str.__new__(cls, value)


class EntityTag(str):
    def __new__(cls, value: str) -> Self:
        if len(value) < 2 or value[0] != '"' or value[-1] != '"':
            raise ValueError(f"invalid entity tag: {value!r}")
        payload = value[1:-1]
        if payload.startswith("blake3:"):
            TreeHash(payload)
        else:
            Blake3(payload)
        return str.__new__(cls, value)


class UndoToken(str):
    def __new__(cls, value: str) -> Self:
        if len(value) != 32 or any(char not in "0123456789abcdef" for char in value):
            raise ValueError(f"invalid undo token: {value!r}")
        return str.__new__(cls, value)


class ManagementToken(str):
    def __new__(cls, value: str) -> Self:
        prefix = "sym_mgmt_"
        if not value.startswith(prefix):
            raise ValueError("invalid management token")
        Blake3(value[len(prefix) :])
        return str.__new__(cls, value)


class CreatorClaim(str):
    def __new__(cls, value: str) -> Self:
        prefix = "sym_claim_"
        if not value.startswith(prefix):
            raise ValueError("invalid creator claim")
        Blake3(value[len(prefix) :])
        return str.__new__(cls, value)


@dataclass(frozen=True, slots=True)
class ApiMetadata:
    artifact: ApiArtifact
    api_version: ApiVersion
    absolute_revision: int
    source_hash: Blake3
    generator_version: ApiVersion
    commit: GitCommit
    dirty: bool


@dataclass(frozen=True, slots=True)
class ApiIdentity:
    api_version: ApiVersion
    absolute_revision: int
    source_hash: Blake3


@dataclass(frozen=True, slots=True)
class ApiVersionDocument:
    api_version: ApiVersion
    absolute_revision: int
    source_hash: Blake3
    commit: GitCommit
    dirty: bool


METADATA_JSON: Final[str] = """{METADATA}"""


def _metadata(source: str) -> ApiMetadata:
    match cast(object, _json.loads(source)):
        case {
            "artifact": str() as artifact,
            "api_version": str() as api_version,
            "absolute_revision": int() as absolute_revision,
            "source_hash": str() as source_hash,
            "generator_version": str() as generator_version,
            "commit": str() as commit,
            "dirty": bool() as dirty,
        }:
            if absolute_revision <= 0:
                raise ValueError("generated API metadata identity is invalid")
            return ApiMetadata(
                artifact=ApiArtifact(artifact),
                api_version=ApiVersion.parse(api_version),
                absolute_revision=absolute_revision,
                source_hash=Blake3(source_hash),
                generator_version=ApiVersion.parse(generator_version),
                commit=GitCommit(commit),
                dirty=dirty,
            )
        case _:
            raise ValueError("generated API metadata does not match its schema")


METADATA: Final[ApiMetadata] = _metadata(METADATA_JSON)

API_VERSION_PARTS: Final[ApiVersion] = METADATA.api_version
API_VERSION: Final[str] = str(API_VERSION_PARTS)
API_REVISION: Final[int] = METADATA.absolute_revision
SOURCE_HASH: Final[Blake3] = METADATA.source_hash
GENERATOR_VERSION_PARTS: Final[ApiVersion] = METADATA.generator_version
GENERATOR_VERSION: Final[str] = str(GENERATOR_VERSION_PARTS)
BUILD_COMMIT: Final[GitCommit] = METADATA.commit
BUILD_DIRTY: Final[bool] = METADATA.dirty


class ByteReader(Protocol):
    def read(self, size: int = -1, /) -> bytes: ...


type ByteSource = bytes | bytearray | memoryview[int] | ByteReader | Path
type Source = ByteSource | str
type AsyncByteSource = bytes | bytearray | memoryview[int] | AsyncIterable[bytes]
type AsyncSource = AsyncByteSource | str
type AsyncSpliceSource = AsyncSource
type Headers = tuple[tuple[str, str], ...]


def _body(value: Source | None) -> bytes | None:
    match value:
        case None:
            return None
        case str():
            return value.encode()
        case bytes():
            return value
        case bytearray() | memoryview():
            return bytes(value)
        case Path():
            return value.read_bytes()
        case _:
            return value.read()


def _sync_body(
    value: Source | None,
) -> tuple[bytes | Iterator[bytes] | None, Callable[[], None]]:
    match value:
        case None:
            return None, lambda: None
        case str():
            return value.encode(), lambda: None
        case bytes():
            return value, lambda: None
        case bytearray() | memoryview():
            return bytes(value), lambda: None
        case Path():
            stream = value.open("rb")
            return _reader_iterator(stream), stream.close
        case _:
            return _reader_iterator(value), lambda: None


def _reader_iterator(
    stream: ByteReader, chunk_size: int = 64 * 1024
) -> Iterator[bytes]:
    while chunk := stream.read(chunk_size):
        yield chunk


def _async_body(value: object) -> bytes | AsyncIterable[bytes] | None:
    match value:
        case None:
            return None
        case str():
            return value.encode()
        case bytes():
            return value
        case bytearray():
            return bytes(value)
        case memoryview():
            return value.tobytes()
        case AsyncIterable():
            return cast(AsyncIterable[bytes], value)
        case _:
            raise TypeError(
                "async request bodies require bytes, text, or AsyncIterable[bytes]"
            )


async def _async_splice_insert(value: object) -> bytes:
    prepared = _async_body(value)
    if prepared is None:
        return b""
    if isinstance(prepared, bytes):
        return prepared
    chunks = bytearray()
    async for chunk in prepared:
        chunks.extend(chunk)
    return bytes(chunks)


def _source_replayable(value: object) -> bool:
    return value is None or isinstance(
        value, str | bytes | bytearray | memoryview | Path
    )


def _header(headers: Headers, name: str) -> str | None:
    folded = name.casefold()
    return next((value for key, value in headers if key.casefold() == folded), None)


def _url(origin: str, *segments: str, trailing: bool = False) -> str:
    path = "/".join(quote(segment, safe="") for segment in segments if segment)
    result = f"{origin.rstrip('/')}/{path}" if path else origin.rstrip("/") + "/"
    return result + "/" if trailing and not result.endswith("/") else result


class HttpMethod(StrEnum):
    GET = "GET"
    HEAD = "HEAD"
    POST = "POST"
    PUT = "PUT"
    DELETE = "DELETE"
    COPY = "COPY"
    MOVE = "MOVE"
    UNDO = "UNDO"
    EXPIRE = "EXPIRE"
    MANAGE = "MANAGE"
    ALIAS = "ALIAS"
    REPLACE = "REPLACE"
    PATCH = "PATCH"


class ApiClientAsset(StrEnum):
    TYPESCRIPT = "symbol.ts"
    JAVASCRIPT = "symbol.js"
    GLOBAL_JAVASCRIPT = "symbol.global.js"
    DECLARATIONS = "symbol.d.ts"
    PYTHON = "symbol.py"


class ApiManual(StrEnum):
    INDEX = "index"
    JAVASCRIPT = "javascript"
    TYPESCRIPT = "typescript"
    PYTHON = "python"
    SHELL = "shell"
    PROTOCOL = "protocol"


class ManagementAction(StrEnum):
    STATUS = "status"
    CLAIM = "claim"
    ROTATE = "rotate"
    RELEASE = "release"


class AliasTargetKind(StrEnum):
    FILE = "file"
    DIRECTORY = "directory"


class DirectoryKind(StrEnum):
    BUILTIN = "builtin"
    SITE = "site"
    DIRECTORY = "directory"
    FILE = "file"
    ALIAS = "alias"


class ExpiryKind(StrEnum):
    SITE = "site"
    FOLDER = "folder"
    FILE = "file"


class UndoKind(StrEnum):
    PUT = "put"
    DELETE_PATH = "delete_path"
    DELETE_SITE = "delete_site"
    COPY = "copy"
    MOVE = "move"
    EXPIRY = "expiry"
    EXPIRE_SWEEP = "expire_sweep"
    PUT_FILE = "put_file"
    ALLOCATE = "allocate"
    REPLACE = "replace"
    SPLICE = "splice"
    ALIAS = "alias"


def _api_manual_path(manual: ApiManual) -> str:
    match manual:
        case ApiManual.INDEX:
            return "/API/"
        case ApiManual.JAVASCRIPT:
            return "/API/JS"
        case ApiManual.TYPESCRIPT:
            return "/API/TS"
        case ApiManual.PYTHON:
            return "/API/PY"
        case ApiManual.SHELL:
            return "/API/SH"
        case ApiManual.PROTOCOL:
            return "/API/CURL"


@dataclass(frozen=True, slots=True)
class ApiRequest:
    method: HttpMethod | str
    url: str
    headers: Headers = ()
    body: Source | None = None
    timeout: float | None = None


@dataclass(frozen=True, slots=True)
class AsyncApiRequest:
    method: HttpMethod | str
    url: str
    headers: Headers = ()
    body: AsyncSource | None = None
    timeout: float | None = None


@dataclass(frozen=True, slots=True)
class RequestAttempt:
    number: int
    started_at: datetime
    finished_at: datetime
    status: int | None
    error: BaseException | None


class SyncResponseStream:
    def __init__(
        self,
        read: Callable[[int], bytes],
        iterate: Callable[[int], Iterator[bytes]],
        close: Callable[[], None],
    ) -> None:
        self.read = read
        self.iterate = iterate
        self.close = close


class AsyncResponseStream:
    def __init__(
        self,
        read: Callable[[], Awaitable[bytes]],
        iterate: Callable[[int], AsyncIterator[bytes]],
        close: Callable[[], Awaitable[None]],
    ) -> None:
        self.read = read
        self.iterate = iterate
        self.close = close


class ApiResponse:
    def __init__(
        self,
        status: int,
        headers: Headers,
        body: bytes = b"",
        *,
        stream: SyncResponseStream | None = None,
        async_stream: AsyncResponseStream | None = None,
        attempts: tuple[RequestAttempt, ...] = (),
        replayable: bool = True,
        retry: Callable[[RetryPolicy | None], ApiResponse] | None = None,
        async_retry: Callable[[RetryPolicy | None], Awaitable[ApiResponse]]
        | None = None,
    ) -> None:
        if stream is not None and async_stream is not None:
            raise ValueError("response cannot have both sync and async streams")
        self.status = status
        self.headers = headers
        self._body = body
        self._stream = stream
        self._async_stream = async_stream
        self._closed = False
        self.attempts = attempts
        self.replayable = replayable
        self._retry = retry
        self._async_retry = async_retry

    def header(self, name: str) -> str | None:
        return _header(self.headers, name)

    @property
    def body(self) -> bytes:
        return self.read()

    def read(self) -> bytes:
        if self._stream is not None:
            self._body += self._stream.read(-1)
            self.close()
        if self._async_stream is not None:
            raise RuntimeError("async response bodies require await response.aread()")
        return self._body

    def iter_bytes(self, chunk_size: int = 64 * 1024) -> Iterator[bytes]:
        if self._body:
            yield self._body
            self._body = b""
        if self._stream is not None:
            try:
                yield from self._stream.iterate(chunk_size)
            finally:
                self.close()
        elif self._async_stream is not None:
            raise RuntimeError("async response bodies require response.aiter_bytes()")

    async def aread(self) -> bytes:
        if self._async_stream is not None:
            self._body += await self._async_stream.read()
            await self.aclose()
        elif self._stream is not None:
            self._body += await asyncio.to_thread(self._stream.read, -1)
            self.close()
        return self._body

    async def aiter_bytes(self, chunk_size: int = 64 * 1024) -> AsyncIterator[bytes]:
        if self._body:
            yield self._body
            self._body = b""
        if self._async_stream is not None:
            try:
                async for chunk in self._async_stream.iterate(chunk_size):
                    yield chunk
            finally:
                await self.aclose()
        elif self._stream is not None:
            for chunk in self.iter_bytes(chunk_size):
                yield chunk

    def text(self) -> str:
        return self.read().decode()

    async def atext(self) -> str:
        return (await self.aread()).decode()

    def json(self) -> Any:
        return _json.loads(self.read())

    async def ajson(self) -> Any:
        return _json.loads(await self.aread())

    def retry(self, policy: RetryPolicy | None = None) -> ApiResponse:
        if not self.replayable:
            raise BodyNotReplayableError
        if self._retry is None:
            raise OperationStateError("successful response cannot be retried")
        return self._retry(policy)

    async def aretry(self, policy: RetryPolicy | None = None) -> ApiResponse:
        if not self.replayable:
            raise BodyNotReplayableError
        if self._async_retry is None:
            raise OperationStateError("successful response cannot be retried")
        return await self._async_retry(policy)

    def abort(self) -> None:
        self.close()

    async def aabort(self) -> None:
        await self.aclose()

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        if self._stream is not None:
            self._stream.close()
            self._stream = None

    async def aclose(self) -> None:
        if self._closed:
            return
        self._closed = True
        if self._async_stream is not None:
            await self._async_stream.close()
            self._async_stream = None
        elif self._stream is not None:
            self._stream.close()
            self._stream = None

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.aclose()


class SyncHttpBackend(Protocol):
    def request(self, request: ApiRequest) -> ApiResponse: ...
    def close(self) -> None: ...


class AsyncHttpBackend(Protocol):
    async def request(self, request: AsyncApiRequest) -> ApiResponse: ...
    async def close(self) -> None: ...


class NetworkRequestError(OSError):
    def __init__(self, cause: BaseException) -> None:
        super().__init__(str(cause) or "network request failed")
        self.cause = cause
        self.attempts: tuple[RequestAttempt, ...] = ()
        self.idempotency_key: IdempotencyKey | None = None
        self.replayable = False
        self._retry: Callable[[RetryPolicy | None], ApiResponse] | None = None
        self._async_retry: (
            Callable[[RetryPolicy | None], Awaitable[ApiResponse]] | None
        ) = None
        self.operation: RetryableOperation[Any] | None = None

    def retry(self, policy: RetryPolicy | None = None) -> Any:
        if self.operation is not None:
            return self.operation.retry(policy)
        if not self.replayable:
            raise BodyNotReplayableError
        if self._retry is None:
            raise OperationStateError("network operation has no synchronous retry")
        return self._retry(policy)

    async def aretry(self, policy: RetryPolicy | None = None) -> Any:
        if self.operation is not None:
            return await self.operation.aretry(policy)
        if not self.replayable:
            raise BodyNotReplayableError
        if self._async_retry is None:
            raise OperationStateError("network operation has no asynchronous retry")
        return await self._async_retry(policy)


class MissingOptionalDependency(ImportError):
    pass


class BodyNotReplayableError(RuntimeError):
    pass


class OperationStateError(RuntimeError):
    pass


class OperationPhase(IntEnum):
    RUNNING = 0
    SUCCEEDED = 1
    FAILED = 2


class RetryableOperation[T](Protocol):
    phase: OperationPhase
    attempts: tuple[RequestAttempt, ...]

    def retry(self, policy: RetryPolicy | None = None) -> T: ...
    async def aretry(self, policy: RetryPolicy | None = None) -> T: ...


class _SyncRetryableOperation[T]:
    def __init__(
        self,
        send: Callable[[], ApiResponse],
        decode: Callable[[ApiResponse], T],
    ) -> None:
        self.phase = OperationPhase.RUNNING
        self.attempts: tuple[RequestAttempt, ...] = ()
        self._send = send
        self._decode = decode
        self._retry_raw: Callable[[RetryPolicy | None], ApiResponse] | None = None
        self._replayable = False
        self._owner: NetworkRequestError | SymbolApiError | None = None

    def _run(self) -> T:
        try:
            return self._complete(self._send())
        except (NetworkRequestError, SymbolApiError) as error:
            self._bind(error)
            raise

    def retry(self, policy: RetryPolicy | None = None) -> T:
        if self.phase is not OperationPhase.FAILED:
            raise OperationStateError("operation is not in a retryable failed state")
        if not self._replayable:
            raise BodyNotReplayableError
        if self._retry_raw is None:
            raise OperationStateError("operation has no synchronous retry")
        self.phase = OperationPhase.RUNNING
        try:
            return self._complete(self._retry_raw(policy))
        except (NetworkRequestError, SymbolApiError) as error:
            self._bind(error)
            raise

    async def aretry(self, policy: RetryPolicy | None = None) -> T:
        del policy
        raise OperationStateError("operation has no asynchronous retry")

    def _complete(self, response: ApiResponse) -> T:
        try:
            value = self._decode(response)
        except SymbolApiError as error:
            self._bind(error)
            raise
        self.phase = OperationPhase.SUCCEEDED
        self.attempts = response.attempts
        if self._owner is not None:
            self._owner.attempts = self.attempts
        return value

    def _bind(self, error: NetworkRequestError | SymbolApiError) -> None:
        self.phase = OperationPhase.FAILED
        self.attempts = error.attempts
        self._replayable = error.replayable
        self._owner = error
        error.operation = self
        self._retry_raw = (
            error._retry
            if isinstance(error, NetworkRequestError)
            else error.response._retry
        )


class _AsyncRetryableOperation[T]:
    def __init__(
        self,
        send: Callable[[], Awaitable[ApiResponse]],
        decode: Callable[[ApiResponse], Awaitable[T]],
    ) -> None:
        self.phase = OperationPhase.RUNNING
        self.attempts: tuple[RequestAttempt, ...] = ()
        self._send = send
        self._decode = decode
        self._retry_raw: (
            Callable[[RetryPolicy | None], Awaitable[ApiResponse]] | None
        ) = None
        self._replayable = False
        self._owner: NetworkRequestError | SymbolApiError | None = None

    async def _run(self) -> T:
        try:
            return await self._complete(await self._send())
        except (NetworkRequestError, SymbolApiError) as error:
            self._bind(error)
            raise

    def retry(self, policy: RetryPolicy | None = None) -> T:
        del policy
        raise OperationStateError("operation has no synchronous retry")

    async def aretry(self, policy: RetryPolicy | None = None) -> T:
        if self.phase is not OperationPhase.FAILED:
            raise OperationStateError("operation is not in a retryable failed state")
        if not self._replayable:
            raise BodyNotReplayableError
        if self._retry_raw is None:
            raise OperationStateError("operation has no asynchronous retry")
        self.phase = OperationPhase.RUNNING
        try:
            return await self._complete(await self._retry_raw(policy))
        except (NetworkRequestError, SymbolApiError) as error:
            self._bind(error)
            raise

    async def _complete(self, response: ApiResponse) -> T:
        try:
            value = await self._decode(response)
        except SymbolApiError as error:
            self._bind(error)
            raise
        self.phase = OperationPhase.SUCCEEDED
        self.attempts = response.attempts
        if self._owner is not None:
            self._owner.attempts = self.attempts
        return value

    def _bind(self, error: NetworkRequestError | SymbolApiError) -> None:
        self.phase = OperationPhase.FAILED
        self.attempts = error.attempts
        self._replayable = error.replayable
        self._owner = error
        error.operation = self
        self._retry_raw = (
            error._async_retry
            if isinstance(error, NetworkRequestError)
            else error.response._async_retry
        )


def _run_typed_sync[T](
    send: Callable[[], ApiResponse],
    decode: Callable[[ApiResponse], T],
) -> T:
    return _SyncRetryableOperation(send, decode)._run()


async def _run_typed_async[T](
    send: Callable[[], Awaitable[ApiResponse]],
    decode: Callable[[ApiResponse], T],
) -> T:
    async def decode_async(response: ApiResponse) -> T:
        return decode(response)

    return await _AsyncRetryableOperation(send, decode_async)._run()


class _StdlibBackend:
    def request(self, request: ApiRequest) -> ApiResponse:
        parsed = urlsplit(request.url)
        connection_type = (
            http.client.HTTPSConnection
            if parsed.scheme == "https"
            else http.client.HTTPConnection
        )
        if parsed.hostname is None:
            raise ValueError("HTTP request URL has no host")
        connection = connection_type(
            parsed.hostname, parsed.port, timeout=request.timeout
        )
        target = parsed.path or "/"
        if parsed.query:
            target += f"?{parsed.query}"
        body, close_body = _sync_body(request.body)
        try:
            connection.request(
                str(request.method),
                target,
                body=body,
                headers=dict(request.headers),
            )
        finally:
            close_body()
        response = connection.getresponse()

        def iterate(chunk_size: int) -> Iterator[bytes]:
            while chunk := response.read(chunk_size):
                yield chunk

        def close() -> None:
            response.close()
            connection.close()

        return ApiResponse(
            response.status,
            tuple(response.getheaders()),
            stream=SyncResponseStream(response.read, iterate, close),
        )

    def close(self) -> None:
        pass


class _SyncAdapter:
    def __init__(
        self, send: Callable[[ApiRequest], ApiResponse], close: Callable[[], None]
    ) -> None:
        self.send = send
        self.closer = close

    def request(self, request: ApiRequest) -> ApiResponse:
        return self.send(request)

    def close(self) -> None:
        self.closer()


class _AsyncAdapter:
    def __init__(
        self,
        send: Callable[[AsyncApiRequest], Awaitable[ApiResponse]],
        close: Callable[[], Awaitable[None]],
    ) -> None:
        self.send = send
        self.closer = close

    async def request(self, request: AsyncApiRequest) -> ApiResponse:
        return await self.send(request)

    async def close(self) -> None:
        await self.closer()


class HttpClient:
    def __init__(self, backend: SyncHttpBackend, *, owned: bool = False) -> None:
        self._backend, self._owned, self._closed = backend, owned, False

    @classmethod
    def stdlib(cls) -> Self:
        return cls(_StdlibBackend(), owned=True)

    @classmethod
    def wrap(cls, backend: SyncHttpBackend) -> Self:
        return cls(backend)

    @classmethod
    def requests(cls, session: requests.Session | None = None) -> Self:
        try:
            import requests as package
        except ImportError as error:
            raise MissingOptionalDependency("requests") from error
        owned = session is None
        session = session or package.Session()

        def send(request: ApiRequest) -> ApiResponse:
            body, close_body = _sync_body(request.body)
            try:
                response = session.request(
                    str(request.method),
                    request.url,
                    headers=dict(request.headers),
                    data=body,
                    timeout=request.timeout,
                    stream=True,
                )
            except package.RequestException as error:
                raise NetworkRequestError(error) from error
            finally:
                close_body()

            def iterate(chunk_size: int) -> Iterator[bytes]:
                yield from response.iter_content(chunk_size=chunk_size)

            return ApiResponse(
                response.status_code,
                tuple(response.headers.items()),
                stream=SyncResponseStream(response.raw.read, iterate, response.close),
            )

        return cls(
            _SyncAdapter(send, session.close if owned else lambda: None), owned=True
        )

    @classmethod
    def urllib3(cls, pool: urllib3.PoolManager | None = None) -> Self:
        try:
            import urllib3 as package
        except ImportError as error:
            raise MissingOptionalDependency("urllib3") from error
        owned = pool is None
        pool = pool or package.PoolManager()

        def send(request: ApiRequest) -> ApiResponse:
            body, close_body = _sync_body(request.body)
            try:
                response = pool.request(
                    str(request.method),
                    request.url,
                    headers=dict(request.headers),
                    body=body,
                    timeout=request.timeout,
                    preload_content=False,
                )
            except package.exceptions.HTTPError as error:
                raise NetworkRequestError(error) from error
            finally:
                close_body()

            def iterate(chunk_size: int) -> Iterator[bytes]:
                while chunk := response.read(chunk_size):
                    yield chunk

            return ApiResponse(
                response.status,
                tuple(response.headers.items()),
                stream=SyncResponseStream(
                    response.read,
                    iterate,
                    response.release_conn,
                ),
            )

        return cls(
            _SyncAdapter(send, pool.clear if owned else lambda: None), owned=True
        )

    @classmethod
    def httpx(cls, client: httpx.Client | None = None) -> Self:
        try:
            import httpx as package
        except ImportError as error:
            raise MissingOptionalDependency("httpx") from error
        owned = client is None
        client = client or package.Client()

        def send(request: ApiRequest) -> ApiResponse:
            body, close_body = _sync_body(request.body)
            try:
                built = client.build_request(
                    str(request.method),
                    request.url,
                    headers=dict(request.headers),
                    content=body,
                    timeout=request.timeout,
                )
                response = client.send(built, stream=True)
            except package.HTTPError as error:
                raise NetworkRequestError(error) from error
            finally:
                close_body()
            return ApiResponse(
                response.status_code,
                tuple(response.headers.items()),
                stream=SyncResponseStream(
                    lambda _size: response.read(),
                    response.iter_bytes,
                    response.close,
                ),
            )

        return cls(
            _SyncAdapter(send, client.close if owned else lambda: None), owned=True
        )

    def request(self, request: ApiRequest) -> ApiResponse:
        if self._closed:
            raise RuntimeError("HTTP client is closed")
        try:
            return self._backend.request(request)
        except (OSError, TimeoutError) as error:
            if isinstance(error, NetworkRequestError):
                raise
            raise NetworkRequestError(error) from error

    def close(self) -> None:
        if not self._closed and self._owned:
            self._backend.close()
        self._closed = True

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        _kind: type[BaseException] | None,
        _error: BaseException | None,
        _traceback: TracebackType | None,
    ) -> None:
        self.close()


class AsyncHttpClient:
    def __init__(self, backend: AsyncHttpBackend, *, owned: bool = False) -> None:
        self._backend, self._owned, self._closed = backend, owned, False

    @classmethod
    def wrap(cls, backend: AsyncHttpBackend) -> Self:
        return cls(backend)

    @classmethod
    def httpx(cls, client: httpx.AsyncClient | None = None) -> Self:
        try:
            import httpx as package
        except ImportError as error:
            raise MissingOptionalDependency("httpx") from error
        owned = client is None
        client = client or package.AsyncClient()

        async def send(request: AsyncApiRequest) -> ApiResponse:
            try:
                built = client.build_request(
                    str(request.method),
                    request.url,
                    headers=dict(request.headers),
                    content=_async_body(request.body),
                    timeout=request.timeout,
                )
                response = await client.send(built, stream=True)
            except package.HTTPError as error:
                raise NetworkRequestError(error) from error
            return ApiResponse(
                response.status_code,
                tuple(response.headers.items()),
                async_stream=AsyncResponseStream(
                    response.aread,
                    response.aiter_bytes,
                    response.aclose,
                ),
            )

        async def close() -> None:
            if owned:
                await client.aclose()

        return cls(_AsyncAdapter(send, close), owned=True)

    @classmethod
    def aiohttp(cls, session: aiohttp.ClientSession | None = None) -> Self:
        try:
            import aiohttp as package
        except ImportError as error:
            raise MissingOptionalDependency("aiohttp") from error
        owned = session is None
        session = session or package.ClientSession()

        async def send(request: AsyncApiRequest) -> ApiResponse:
            timeout = (
                package.ClientTimeout(total=request.timeout)
                if request.timeout is not None
                else package.ClientTimeout()
            )
            try:
                response = await session.request(
                    str(request.method),
                    request.url,
                    headers=dict(request.headers),
                    data=_async_body(request.body),
                    timeout=timeout,
                )
            except (TimeoutError, package.ClientError) as error:
                raise NetworkRequestError(error) from error

            async def close_response() -> None:
                response.close()

            return ApiResponse(
                response.status,
                tuple(response.headers.items()),
                async_stream=AsyncResponseStream(
                    response.read,
                    response.content.iter_chunked,
                    close_response,
                ),
            )

        async def close() -> None:
            if owned:
                await session.close()

        return cls(_AsyncAdapter(send, close), owned=True)

    async def request(self, request: AsyncApiRequest) -> ApiResponse:
        if self._closed:
            raise RuntimeError("HTTP client is closed")
        try:
            return await self._backend.request(request)
        except (OSError, TimeoutError) as error:
            if isinstance(error, NetworkRequestError):
                raise
            raise NetworkRequestError(error) from error

    async def close(self) -> None:
        if not self._closed and self._owned:
            await self._backend.close()
        self._closed = True

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        _kind: type[BaseException] | None,
        _error: BaseException | None,
        _traceback: TracebackType | None,
    ) -> None:
        await self.close()


class Charset(StrEnum):
    UTF8 = "utf-8"
    US_ASCII = "us-ascii"
    ISO_8859_1 = "iso-8859-1"


def _is_media_token(value: str) -> bool:
    allowed = "!#$%&'*+-.^_`|~"
    return bool(value) and all(
        char.isascii() and (char.isalnum() or char in allowed) for char in value
    )


@dataclass(frozen=True, slots=True)
class MediaType:
    type: str
    subtype: str
    parameters: tuple[tuple[str, str], ...] = ()

    def __post_init__(self) -> None:
        if not _is_media_token(self.type) or not _is_media_token(self.subtype):
            raise ValueError("invalid media type")
        parameters = tuple(
            sorted((name.casefold(), value) for name, value in self.parameters)
        )
        if len({name for name, _ in parameters}) != len(parameters):
            raise ValueError("duplicate media parameter")
        object.__setattr__(self, "type", self.type.casefold())
        object.__setattr__(self, "subtype", self.subtype.casefold())
        object.__setattr__(self, "parameters", parameters)

    @classmethod
    def parse(cls, value: str) -> Self:
        parts = [part.strip() for part in value.split(";")]
        top, separator, subtype = parts[0].partition("/")
        if not separator:
            raise ValueError("invalid media type")
        parameters: list[tuple[str, str]] = []
        for part in parts[1:]:
            name, separator, parameter = part.partition("=")
            if not separator:
                raise ValueError("invalid media parameter")
            parameter = parameter.strip()
            if parameter.startswith('"') and parameter.endswith('"'):
                parameter = parameter[1:-1].replace(r"\\", "\\").replace(r"\"", '"')
            parameters.append((name.strip(), parameter))
        return cls(top, subtype, tuple(parameters))

    @classmethod
    def application(cls, subtype: str) -> Self:
        return cls("application", subtype)

    @classmethod
    def text(cls, subtype: str, charset: str = Charset.UTF8) -> Self:
        return cls("text", subtype, (("charset", str(charset)),))

    @classmethod
    def image(cls, subtype: str) -> Self:
        return cls("image", subtype)

    @property
    def essence(self) -> str:
        return f"{self.type}/{self.subtype}"

    @property
    def charset(self) -> str | None:
        return self.parameter("charset")

    def parameter(self, name: str) -> str | None:
        return next(
            (value for key, value in self.parameters if key == name.casefold()), None
        )

    def with_parameter(self, name: str, value: str) -> Self:
        retained = tuple(
            (key, item) for key, item in self.parameters if key != name.casefold()
        )
        return type(self)(self.type, self.subtype, (*retained, (name, value)))

    def __str__(self) -> str:
        suffix = "".join(
            f"; {name}={_quote_parameter(value)}" for name, value in self.parameters
        )
        return self.essence + suffix


def _quote_parameter(value: str) -> str:
    allowed = "!#$%&'*+-.^_`|~"
    if value and all(
        char.isascii() and (char.isalnum() or char in allowed) for char in value
    ):
        return value
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


@dataclass(frozen=True, slots=True)
class ContentFormat:
    media_type: MediaType
    extensions: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "extensions",
            tuple(item.lstrip(".").casefold() for item in self.extensions),
        )


class MediaTypes:
    BINARY = MediaType.application("octet-stream")
    JSON = MediaType.application("json")
    TEXT = MediaType.text("plain")
    HTML = MediaType.text("html")
    CSS = MediaType.text("css")
    JAVASCRIPT = MediaType.text("javascript")
    MARKDOWN = MediaType.text("markdown")
    XML = MediaType.application("xml")
    PDF = MediaType.application("pdf")
    PNG = MediaType.image("png")
    JPEG = MediaType.image("jpeg")
    GIF = MediaType.image("gif")
    WEBP = MediaType.image("webp")
    SVG = MediaType.image("svg+xml")
    ZIP = MediaType.application("zip")
    GZIP = MediaType.application("gzip")
    WASM = MediaType.application("wasm")


class ContentFormats:
    BINARY = ContentFormat(MediaTypes.BINARY)
    JSON = ContentFormat(MediaTypes.JSON, ("json",))
    TEXT = ContentFormat(MediaTypes.TEXT, ("txt",))
    HTML = ContentFormat(MediaTypes.HTML, ("html", "htm"))
    CSS = ContentFormat(MediaTypes.CSS, ("css",))
    JAVASCRIPT = ContentFormat(MediaTypes.JAVASCRIPT, ("js", "mjs"))
    MARKDOWN = ContentFormat(MediaTypes.MARKDOWN, ("md", "markdown"))
    PDF = ContentFormat(MediaTypes.PDF, ("pdf",))
    PNG = ContentFormat(MediaTypes.PNG, ("png",))
    JPEG = ContentFormat(MediaTypes.JPEG, ("jpg", "jpeg"))
    SVG = ContentFormat(MediaTypes.SVG, ("svg",))
    WASM = ContentFormat(MediaTypes.WASM, ("wasm",))


@dataclass(frozen=True, slots=True)
class GeneratedName:
    prefix: str = ""
    extension: str = ""
    suffix: str = ""


class RetryJitter(StrEnum):
    NONE = "none"
    FULL = "full"


@dataclass(frozen=True, slots=True)
class RetryPolicy:
    max_attempts: int = 1
    initial_delay: float = 0.25
    maximum_delay: float = 8.0
    backoff_factor: float = 2.0
    jitter: RetryJitter = RetryJitter.NONE
    honor_retry_after: bool = True
    retry_network_errors: bool = False
    retry_statuses: tuple[int, ...] = ()

    def __post_init__(self) -> None:
        if (
            self.max_attempts < 1
            or self.initial_delay < 0
            or self.maximum_delay < self.initial_delay
            or self.backoff_factor < 1
            or not all(100 <= status <= 599 for status in self.retry_statuses)
        ):
            raise ValueError("invalid retry policy")


class RetryPolicies:
    DISABLED = RetryPolicy()
    DEFAULT = RetryPolicy(
        max_attempts=4,
        jitter=RetryJitter.FULL,
        retry_network_errors=True,
        retry_statuses=(408, 425, 429, 500, 502, 503, 504),
    )


@dataclass(frozen=True, slots=True)
class RequestOptions:
    token: ManagementToken | None = None
    headers: Headers = ()
    timeout: float | None = None


@dataclass(frozen=True, slots=True)
class MutationOptions(RequestOptions):
    if_match: TreeHash | None = None
    idempotency_key: IdempotencyKey | None = None


@dataclass(frozen=True, slots=True)
class CreateFileOptions(MutationOptions):
    media_type: MediaType | str | None = None
    name: GeneratedName | Callable[[ProposedFileName], str | Awaitable[str]] = (
        GeneratedName()
    )


@dataclass(frozen=True, slots=True)
class PublishOptions(MutationOptions):
    media_type: MediaType | str = MediaTypes.BINARY
    filename: str | None = None
    unpack: bool = False
    managed: bool = False


@dataclass(frozen=True, slots=True)
class ProposedFileName:
    folder: SitePath
    default_name: SitePath
    hash: TreeHash
    size: int
    token: AllocationToken


@dataclass(frozen=True, slots=True)
class ByteSplice:
    offset: int
    delete_bytes: int
    insert: ByteSource = b""


@dataclass(frozen=True, slots=True)
class AsyncByteSplice:
    offset: int
    delete_bytes: int
    insert: AsyncSpliceSource = b""


@dataclass(frozen=True, slots=True)
class RelativeExpiry:
    duration: str


@dataclass(frozen=True, slots=True)
class AbsoluteExpiry:
    at: str | datetime


@dataclass(frozen=True, slots=True)
class DecayExpiry:
    min_age: str
    max_age: str
    max_size: str | int
    power: float


@dataclass(frozen=True, slots=True)
class NeverExpiry:
    pass


type Expiry = RelativeExpiry | AbsoluteExpiry | DecayExpiry | NeverExpiry


@dataclass(frozen=True, slots=True)
class UndoReceipt:
    token: UndoToken
    expires_at: datetime


@dataclass(frozen=True, slots=True)
class UndoEntry:
    token: UndoToken
    kind: UndoKind
    description: str
    created_at: datetime
    expires_at: datetime
    remaining_seconds: int


@dataclass(frozen=True, slots=True)
class UndoStack:
    site: SiteName
    entries: tuple[UndoEntry, ...]


@dataclass(frozen=True, slots=True)
class MutationReceipt:
    status: int
    location: ResourceUrl
    etag: EntityTag
    content_revision: int
    changed: bool
    undo: UndoReceipt | None
    idempotency_key: IdempotencyKey | None
    creator_claim: CreatorClaim | None
    management_token: ManagementToken | None
    response: ApiResponse


@dataclass(frozen=True, slots=True)
class DeleteReceipt:
    status: int
    undo: UndoReceipt
    response: ApiResponse


@dataclass(frozen=True, slots=True)
class AllocationReceipt:
    mutation: MutationReceipt
    site: SiteName
    path: SitePath
    hash: TreeHash
    blob_url: ResourceUrl


@dataclass(frozen=True, slots=True)
class AliasReceipt:
    mutation: MutationReceipt
    path: SitePath
    target: SitePath
    target_kind: AliasTargetKind | None
    dangling: bool
    resolved_hash: TreeHash | None
    size: int | None


@dataclass(frozen=True, slots=True)
class AliasDefinition:
    path: SitePath
    target: SitePath


@dataclass(frozen=True, slots=True)
class FileEntry:
    path: SitePath
    hash: TreeHash
    size: int


@dataclass(frozen=True, slots=True)
class AliasInventoryEntry:
    path: SitePath
    target: SitePath
    target_kind: AliasTargetKind | None
    dangling: bool
    resolved_hash: TreeHash | None
    size: int | None


@dataclass(frozen=True, slots=True)
class SiteEvent:
    kind: str
    at: datetime
    files: int


@dataclass(frozen=True, slots=True)
class FileInventory:
    site: SiteName
    created_at: datetime | None
    updated_at: datetime
    content_revision: int
    tree_hash: TreeHash
    etag: EntityTag
    events: tuple[SiteEvent, ...]
    files: tuple[FileEntry, ...]
    aliases: tuple[AliasInventoryEntry, ...]


@dataclass(frozen=True, slots=True)
class DirectoryEntry:
    kind: DirectoryKind
    name: str
    files: int | None
    bytes: int
    target: str | None
    target_kind: AliasTargetKind | None
    dangling: bool | None


@dataclass(frozen=True, slots=True)
class DirectoryListing:
    path: str
    files: int
    aliases: int
    bytes: int
    entries: tuple[DirectoryEntry, ...]


@dataclass(frozen=True, slots=True)
class ExpiryTarget:
    site: str
    path: str | None
    kind: ExpiryKind


@dataclass(frozen=True, slots=True)
class ExpiryCap:
    kind: ExpiryKind
    path: str | None
    expires_at: datetime


@dataclass(frozen=True, slots=True)
class RelativeExpiryPolicy:
    retention_seconds: int
    expires_at: datetime


@dataclass(frozen=True, slots=True)
class AbsoluteExpiryPolicy:
    expires_at: datetime


@dataclass(frozen=True, slots=True)
class DecayExpiryPolicy:
    min_age_seconds: int
    max_age_seconds: int
    max_size_bytes: int
    power: float
    retention_seconds: int
    expires_at: datetime


type OwnedExpiryPolicy = RelativeExpiryPolicy | AbsoluteExpiryPolicy | DecayExpiryPolicy


@dataclass(frozen=True, slots=True)
class ExpiryLimit:
    kind: ExpiryKind
    path: str | None


@dataclass(frozen=True, slots=True)
class ExpiryReport:
    target: ExpiryTarget
    size: int
    refreshed_at: datetime | None
    own_policy: OwnedExpiryPolicy | None
    inherited_caps: tuple[ExpiryCap, ...]
    effective_expires_at: datetime | None
    remaining_seconds: int | None
    limited_by: ExpiryLimit | None


@dataclass(frozen=True, slots=True)
class ExpirySiteReport:
    site: str
    entries: tuple[ExpiryReport, ...]


@dataclass(frozen=True, slots=True)
class SizeDistribution:
    min: float | None
    p25: float | None
    median: float | None
    mean: float | None
    p75: float | None
    max: float | None
    iqr: float | None
    stddev: float | None


@dataclass(frozen=True, slots=True)
class CacheStats:
    hits: int
    misses: int
    evictions: int


@dataclass(frozen=True, slots=True)
class ReaderStats:
    operations: int
    waits: int
    wait_micros: int
    query_micros: int


@dataclass(frozen=True, slots=True)
class ServingStats:
    cache: CacheStats
    readers: ReaderStats


@dataclass(frozen=True, slots=True)
class SymbolStats:
    sites: int
    files: int
    aliases: int
    blobs: int
    bytes: int
    logical_bytes: int
    saved_bytes: int
    saved_fraction: float
    file_sizes: SizeDistribution
    blob_sizes: SizeDistribution
    serving: ServingStats


def _decode_symbol_stats(response: ApiResponse) -> SymbolStats:
    if response.status != 200:
        _raise(response)
    return _symbol_stats(_json_object(response), response)


async def _decode_symbol_stats_async(response: ApiResponse) -> SymbolStats:
    return _decode_symbol_stats(response)


@dataclass(frozen=True, slots=True)
class ManagementStatus:
    managed: bool


class SymbolApiError(RuntimeError):
    def __init__(
        self, response: ApiResponse, idempotency_key: str | None = None
    ) -> None:
        super().__init__(
            response.body.decode(errors="replace").rstrip() or f"HTTP {response.status}"
        )
        self.status = response.status
        self.response = response
        self.idempotency_key = idempotency_key
        self.attempts = response.attempts
        self.replayable = response.replayable
        self.operation: RetryableOperation[Any] | None = None

    def retry(self, policy: RetryPolicy | None = None) -> Any:
        if self.operation is not None:
            return self.operation.retry(policy)
        return self.response.retry(policy)

    async def aretry(self, policy: RetryPolicy | None = None) -> Any:
        if self.operation is not None:
            return await self.operation.aretry(policy)
        return await self.response.aretry(policy)

    def abort(self) -> None:
        self.response.abort()

    async def aabort(self) -> None:
        await self.response.aabort()


class MalformedResponseError(SymbolApiError):
    pass


class ValidationError(SymbolApiError):
    pass


class UnauthorizedError(SymbolApiError):
    def __init__(
        self, response: ApiResponse, idempotency_key: IdempotencyKey | None = None
    ) -> None:
        super().__init__(response, idempotency_key)
        challenge = response.header("WWW-Authenticate")
        if challenge is None:
            raise MalformedResponseError(response, idempotency_key)
        self.challenge = challenge


class ForbiddenError(SymbolApiError):
    pass


class NotFoundError(SymbolApiError):
    pass


class MethodNotAllowedError(SymbolApiError):
    pass


class ConflictError(SymbolApiError):
    pass


class PreconditionFailedError(SymbolApiError):
    def __init__(
        self, response: ApiResponse, idempotency_key: IdempotencyKey | None = None
    ) -> None:
        super().__init__(response, idempotency_key)
        etag = response.header("ETag")
        revision = response.header("Content-Revision")
        if (
            etag is None
            or revision is None
            or not revision.isascii()
            or not revision.isdecimal()
        ):
            raise MalformedResponseError(response, idempotency_key)
        self.etag = etag
        self.content_revision = int(revision)


class PayloadTooLargeError(SymbolApiError):
    pass


class RangeNotSatisfiableError(SymbolApiError):
    def __init__(
        self, response: ApiResponse, idempotency_key: IdempotencyKey | None = None
    ) -> None:
        super().__init__(response, idempotency_key)
        content_range = response.header("Content-Range")
        if content_range is None:
            raise MalformedResponseError(response, idempotency_key)
        self.content_range = content_range


class ServerError(SymbolApiError):
    pass


class UnexpectedResponseError(SymbolApiError):
    pass


class IncompatibleApiVersionError(SymbolApiError):
    pass


def _raise(response: ApiResponse, key: str | None = None) -> None:
    error_type: type[SymbolApiError]
    match response.status:
        case 400:
            error_type = ValidationError
        case 401:
            error_type = UnauthorizedError
        case 403:
            error_type = ForbiddenError
        case 404:
            error_type = NotFoundError
        case 405:
            error_type = MethodNotAllowedError
        case 409:
            error_type = ConflictError
        case 412:
            error_type = PreconditionFailedError
        case 413:
            error_type = PayloadTooLargeError
        case 416:
            error_type = RangeNotSatisfiableError
        case status if 500 <= status <= 599:
            error_type = ServerError
        case _:
            error_type = UnexpectedResponseError
    raise error_type(response, key)


def _json_object(response: ApiResponse) -> Mapping[str, object]:
    value = cast(object, response.json())
    match value:
        case dict():
            unknown = cast(Mapping[object, object], value)
            if not all(isinstance(key, str) for key in unknown):
                raise MalformedResponseError(response)
            return cast(Mapping[str, object], unknown)
        case _:
            raise MalformedResponseError(response)


def _exact_object(
    value: object,
    fields: tuple[str, ...],
    response: ApiResponse,
) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise MalformedResponseError(response)
    unknown = cast(Mapping[object, object], value)
    if not all(isinstance(key, str) for key in unknown):
        raise MalformedResponseError(response)
    result = cast(Mapping[str, object], unknown)
    if set(result) != set(fields):
        raise MalformedResponseError(response)
    return result


def _string(value: object, response: ApiResponse) -> str:
    if not isinstance(value, str):
        raise MalformedResponseError(response)
    return value


def _integer(value: object, response: ApiResponse) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise MalformedResponseError(response)
    return value


def _number(value: object, response: ApiResponse) -> float:
    if not isinstance(value, int | float) or isinstance(value, bool):
        raise MalformedResponseError(response)
    result = float(value)
    if not math.isfinite(result):
        raise MalformedResponseError(response)
    return result


def _nullable_number(value: object, response: ApiResponse) -> float | None:
    return None if value is None else _number(value, response)


def _boolean(value: object, response: ApiResponse) -> bool:
    if not isinstance(value, bool):
        raise MalformedResponseError(response)
    return value


def _enum_value[T: StrEnum](
    enum_type: type[T], value: object, response: ApiResponse
) -> T:
    try:
        return enum_type(_string(value, response))
    except ValueError as error:
        raise MalformedResponseError(response) from error


def _array(value: object, response: ApiResponse) -> list[object]:
    if not isinstance(value, list):
        raise MalformedResponseError(response)
    return cast(list[object], value)


def _symbol_stats(value: Mapping[str, object], response: ApiResponse) -> SymbolStats:
    value = _exact_object(
        value,
        (
            "sites",
            "files",
            "aliases",
            "blobs",
            "bytes",
            "logical_bytes",
            "saved_bytes",
            "saved_fraction",
            "file_sizes",
            "blob_sizes",
            "serving",
        ),
        response,
    )
    serving = _exact_object(value["serving"], ("cache", "readers"), response)
    cache = _exact_object(serving["cache"], ("hits", "misses", "evictions"), response)
    readers = _exact_object(
        serving["readers"],
        ("operations", "waits", "wait_micros", "query_micros"),
        response,
    )
    return SymbolStats(
        sites=_integer(value.get("sites"), response),
        files=_integer(value.get("files"), response),
        aliases=_integer(value.get("aliases"), response),
        blobs=_integer(value.get("blobs"), response),
        bytes=_integer(value.get("bytes"), response),
        logical_bytes=_integer(value.get("logical_bytes"), response),
        saved_bytes=_integer(value.get("saved_bytes"), response),
        saved_fraction=_number(value.get("saved_fraction"), response),
        file_sizes=_size_distribution(value["file_sizes"], response),
        blob_sizes=_size_distribution(value["blob_sizes"], response),
        serving=ServingStats(
            cache=CacheStats(
                hits=_integer(cache["hits"], response),
                misses=_integer(cache["misses"], response),
                evictions=_integer(cache["evictions"], response),
            ),
            readers=ReaderStats(
                operations=_integer(readers["operations"], response),
                waits=_integer(readers["waits"], response),
                wait_micros=_integer(readers["wait_micros"], response),
                query_micros=_integer(readers["query_micros"], response),
            ),
        ),
    )


def _size_distribution(value: object, response: ApiResponse) -> SizeDistribution:
    distribution = _exact_object(
        value,
        ("min", "p25", "median", "mean", "p75", "max", "iqr", "stddev"),
        response,
    )
    return SizeDistribution(
        min=_nullable_number(distribution["min"], response),
        p25=_nullable_number(distribution["p25"], response),
        median=_nullable_number(distribution["median"], response),
        mean=_nullable_number(distribution["mean"], response),
        p75=_nullable_number(distribution["p75"], response),
        max=_nullable_number(distribution["max"], response),
        iqr=_nullable_number(distribution["iqr"], response),
        stddev=_nullable_number(distribution["stddev"], response),
    )


def _api_version_document(response: ApiResponse) -> ApiVersionDocument:
    if response.status != 200:
        _raise(response)
    value = _exact_object(
        _json_object(response),
        ("api_version", "absolute_revision", "source_hash", "commit", "dirty"),
        response,
    )
    try:
        document = ApiVersionDocument(
            api_version=ApiVersion.parse(_string(value["api_version"], response)),
            absolute_revision=_integer(value["absolute_revision"], response),
            source_hash=Blake3(_string(value["source_hash"], response)),
            commit=GitCommit(_string(value["commit"], response)),
            dirty=_boolean(value["dirty"], response),
        )
    except ValueError as error:
        raise MalformedResponseError(response) from error
    if document.absolute_revision <= 0:
        raise MalformedResponseError(response)
    return document


def _response_identity(response: ApiResponse) -> ApiIdentity:
    version = response.header("Symbol-API-Version")
    revision = response.header("Symbol-API-Revision")
    source_hash = response.header("Symbol-API-Source-Hash")
    if version is None or revision is None or source_hash is None:
        raise MalformedResponseError(response)
    try:
        identity = ApiIdentity(
            api_version=ApiVersion.parse(version),
            absolute_revision=int(revision),
            source_hash=Blake3(source_hash),
        )
    except ValueError as error:
        raise MalformedResponseError(response) from error
    if identity.absolute_revision <= 0:
        raise MalformedResponseError(response)
    if identity.api_version.major != API_VERSION_PARTS.major:
        raise IncompatibleApiVersionError(response)
    if (
        identity.api_version == API_VERSION_PARTS
        and identity.absolute_revision == API_REVISION
        and identity.source_hash != SOURCE_HASH
    ):
        raise IncompatibleApiVersionError(response)
    return identity


def _observe_identity(
    observed: ApiIdentity | None,
    current: ApiIdentity,
    response: ApiResponse,
) -> ApiIdentity:
    if observed is None:
        return current
    if (
        current.api_version < observed.api_version
        or current.absolute_revision < observed.absolute_revision
        or (
            current.api_version > observed.api_version
            and current.absolute_revision <= observed.absolute_revision
        )
        or (
            current.api_version == observed.api_version
            and current.absolute_revision == observed.absolute_revision
            and current.source_hash != observed.source_hash
        )
    ):
        raise IncompatibleApiVersionError(response)
    return current


def _raw_hash(response: ApiResponse) -> Blake3:
    if response.status != 200:
        _raise(response)
    value = response.text().strip()
    if len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
        raise UnexpectedResponseError(response)
    return Blake3(value)


def _file_inventory(response: ApiResponse) -> FileInventory:
    if response.status != 200:
        _raise(response)
    value = _json_object(response)
    etag = response.header("ETag")
    if etag is None:
        raise MalformedResponseError(response)
    try:
        parsed_etag = EntityTag(etag)
    except ValueError as error:
        raise MalformedResponseError(response) from error
    return FileInventory(
        _string(value["site"], response),
        _date(value["created_at"], response),
        _date(value["updated_at"], response),
        _integer(value["content_revision"], response),
        TreeHash(_string(value["tree_hash"], response)),
        parsed_etag,
        tuple(
            _site_event(item, response) for item in _array(value["events"], response)
        ),
        tuple(_file_entry(item, response) for item in _array(value["files"], response)),
        tuple(
            _alias_inventory_entry(item, response)
            for item in _array(value["aliases"], response)
        ),
    )


def _site_event(value: object, response: ApiResponse) -> SiteEvent:
    entry = _exact_object(value, ("kind", "at", "files"), response)
    kind = _string(entry["kind"], response)
    if kind not in ("created", "publish", "rename", "restore"):
        raise MalformedResponseError(response)
    return SiteEvent(
        kind=kind,
        at=_date(entry["at"], response),
        files=_integer(entry["files"], response),
    )


def _file_entry(value: object, response: ApiResponse) -> FileEntry:
    entry = _exact_object(value, ("path", "hash", "size"), response)
    return FileEntry(
        path=_string(entry["path"], response),
        hash=TreeHash(_string(entry["hash"], response)),
        size=_integer(entry["size"], response),
    )


def _alias_inventory_entry(value: object, response: ApiResponse) -> AliasInventoryEntry:
    entry = _exact_object(
        value,
        ("path", "target", "target_kind", "dangling", "resolved_hash", "size"),
        response,
    )
    return AliasInventoryEntry(
        path=_string(entry["path"], response),
        target=_string(entry["target"], response),
        target_kind=(
            _enum_value(AliasTargetKind, entry["target_kind"], response)
            if entry["target_kind"] is not None
            else None
        ),
        dangling=_boolean(entry["dangling"], response),
        resolved_hash=(
            TreeHash(_string(entry["resolved_hash"], response))
            if entry["resolved_hash"] is not None
            else None
        ),
        size=(_integer(entry["size"], response) if entry["size"] is not None else None),
    )


def _date(value: object, response: ApiResponse) -> datetime | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise MalformedResponseError(response)
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(UTC)
    except ValueError as error:
        raise MalformedResponseError(response) from error


def _expiry_policy(value: object, response: ApiResponse) -> OwnedExpiryPolicy:
    policy = _exact_object(
        value,
        (
            "mode",
            "min_age_seconds",
            "max_age_seconds",
            "max_size_bytes",
            "power",
            "retention_seconds",
            "expires_at",
        ),
        response,
    )
    mode = _string(policy["mode"], response)
    expires_at = _date(policy["expires_at"], response)
    if expires_at is None:
        raise MalformedResponseError(response)
    match mode:
        case "relative":
            if any(
                policy[field] is not None
                for field in (
                    "min_age_seconds",
                    "max_age_seconds",
                    "max_size_bytes",
                    "power",
                )
            ):
                raise MalformedResponseError(response)
            return RelativeExpiryPolicy(
                retention_seconds=_integer(policy["retention_seconds"], response),
                expires_at=expires_at,
            )
        case "absolute":
            if any(
                policy[field] is not None
                for field in (
                    "min_age_seconds",
                    "max_age_seconds",
                    "max_size_bytes",
                    "power",
                    "retention_seconds",
                )
            ):
                raise MalformedResponseError(response)
            return AbsoluteExpiryPolicy(expires_at=expires_at)
        case "decay":
            return DecayExpiryPolicy(
                min_age_seconds=_integer(policy["min_age_seconds"], response),
                max_age_seconds=_integer(policy["max_age_seconds"], response),
                max_size_bytes=_integer(policy["max_size_bytes"], response),
                power=_number(policy["power"], response),
                retention_seconds=_integer(policy["retention_seconds"], response),
                expires_at=expires_at,
            )
        case _:
            raise MalformedResponseError(response)


def _expiry_cap(value: object, response: ApiResponse) -> ExpiryCap:
    cap = _exact_object(value, ("kind", "path", "expires_at"), response)
    expires_at = _date(cap["expires_at"], response)
    if expires_at is None:
        raise MalformedResponseError(response)
    return ExpiryCap(
        kind=_enum_value(ExpiryKind, cap["kind"], response),
        path=_string(cap["path"], response) if cap["path"] is not None else None,
        expires_at=expires_at,
    )


def _expiry_limit(value: object, response: ApiResponse) -> ExpiryLimit:
    limit = _exact_object(value, ("kind", "path"), response)
    return ExpiryLimit(
        kind=_enum_value(ExpiryKind, limit["kind"], response),
        path=(_string(limit["path"], response) if limit["path"] is not None else None),
    )


def _expiry_report(value: object, response: ApiResponse) -> ExpiryReport:
    value = _exact_object(
        value,
        (
            "target",
            "size",
            "refreshed_at",
            "own_policy",
            "inherited_caps",
            "effective_expires_at",
            "remaining_seconds",
            "limited_by",
        ),
        response,
    )
    target = _exact_object(value["target"], ("site", "path", "kind"), response)
    own_policy = value.get("own_policy")
    limited_by = value.get("limited_by")
    return ExpiryReport(
        target=ExpiryTarget(
            site=_string(target["site"], response),
            path=(
                _string(target["path"], response)
                if target["path"] is not None
                else None
            ),
            kind=_enum_value(ExpiryKind, target["kind"], response),
        ),
        size=_integer(value["size"], response),
        refreshed_at=_date(value.get("refreshed_at"), response),
        own_policy=(
            _expiry_policy(own_policy, response) if own_policy is not None else None
        ),
        inherited_caps=tuple(
            _expiry_cap(cap, response)
            for cap in _array(value.get("inherited_caps"), response)
        ),
        effective_expires_at=_date(value.get("effective_expires_at"), response),
        remaining_seconds=(
            _integer(value["remaining_seconds"], response)
            if value.get("remaining_seconds") is not None
            else None
        ),
        limited_by=_expiry_limit(limited_by, response)
        if limited_by is not None
        else None,
    )


def _undo_stack(response: ApiResponse) -> UndoStack:
    if response.status != 200:
        _raise(response)
    value = _exact_object(_json_object(response), ("site", "entries"), response)
    return UndoStack(
        site=_string(value["site"], response),
        entries=tuple(
            _undo_entry(entry, response) for entry in _array(value["entries"], response)
        ),
    )


def _undo_entry(value: object, response: ApiResponse) -> UndoEntry:
    entry = _exact_object(
        value,
        (
            "token",
            "kind",
            "description",
            "created_at",
            "expires_at",
            "remaining_seconds",
        ),
        response,
    )
    created_at = _date(entry["created_at"], response)
    expires_at = _date(entry["expires_at"], response)
    if created_at is None or expires_at is None:
        raise MalformedResponseError(response)
    return UndoEntry(
        token=UndoToken(_string(entry["token"], response)),
        kind=_enum_value(UndoKind, entry["kind"], response),
        description=_string(entry["description"], response),
        created_at=created_at,
        expires_at=expires_at,
        remaining_seconds=_integer(entry["remaining_seconds"], response),
    )


def _management_status(response: ApiResponse) -> ManagementStatus:
    value = _exact_object(_json_object(response), ("managed",), response)
    return ManagementStatus(_boolean(value["managed"], response))


def _mutation(
    response: ApiResponse,
    key: IdempotencyKey | None = None,
    expected_changed: bool | None = None,
) -> MutationReceipt:
    if response.status < 200 or response.status >= 300:
        _raise(response, key)
    token = response.header("Undo-Token")
    expires = response.header("Undo-Expires")
    if (token is None) != (expires is None):
        raise MalformedResponseError(response, key)
    undo = None
    if token is not None and expires is not None:
        try:
            undo = UndoReceipt(
                UndoToken(token),
                datetime.fromisoformat(expires.replace("Z", "+00:00")).astimezone(UTC),
            )
        except ValueError as error:
            raise MalformedResponseError(response, key) from error
    location = response.header("Location")
    etag = response.header("ETag")
    revision = response.header("Content-Revision")
    if (
        location is None
        or etag is None
        or revision is None
        or not revision.isascii()
        or not revision.isdecimal()
    ):
        raise MalformedResponseError(response, key)
    try:
        parsed_etag = EntityTag(etag)
    except ValueError as error:
        raise MalformedResponseError(response, key) from error
    content_type = response.header("Content-Type") or ""
    if content_type.casefold().startswith("application/json"):
        changed = _boolean(_json_object(response).get("changed"), response)
    else:
        text = response.text()
        if "changed: true" in text:
            changed = True
        elif "changed: false" in text:
            changed = False
        elif expected_changed is not None:
            changed = expected_changed
        else:
            raise MalformedResponseError(response, key)
    if changed != (undo is not None):
        raise MalformedResponseError(response, key)
    return MutationReceipt(
        status=response.status,
        location=location,
        etag=parsed_etag,
        content_revision=int(revision),
        changed=changed,
        undo=undo,
        idempotency_key=key,
        creator_claim=(
            CreatorClaim(claim)
            if (claim := response.header("Creator-Claim")) is not None
            else None
        ),
        management_token=(
            ManagementToken(management)
            if (management := response.header("Management-Token")) is not None
            else None
        ),
        response=response,
    )


def _delete_receipt(response: ApiResponse) -> DeleteReceipt:
    if response.status != 200:
        _raise(response)
    token = response.header("Undo-Token")
    expires = response.header("Undo-Expires")
    if token is None or expires is None or not response.text().startswith("deleted "):
        raise MalformedResponseError(response)
    try:
        undo = UndoReceipt(
            UndoToken(token),
            datetime.fromisoformat(expires.replace("Z", "+00:00")).astimezone(UTC),
        )
    except ValueError as error:
        raise MalformedResponseError(response) from error
    return DeleteReceipt(status=200, undo=undo, response=response)


def _allocation(response: ApiResponse, key: str) -> AllocationReceipt:
    mutation = _mutation(response, key)
    value = _json_object(response)
    return AllocationReceipt(
        mutation=mutation,
        site=_string(value["site"], response),
        path=_string(value["path"], response),
        hash=TreeHash(_string(value["hash"], response)),
        blob_url=_string(value["blob_url"], response),
    )


def _alias(response: ApiResponse, key: str) -> AliasReceipt:
    mutation = _mutation(response, key)
    value = _json_object(response)
    return AliasReceipt(
        mutation=mutation,
        path=_string(value["path"], response),
        target=_string(value["target"], response),
        target_kind=(
            _enum_value(AliasTargetKind, value["target_kind"], response)
            if value.get("target_kind") is not None
            else None
        ),
        dangling=_boolean(value["dangling"], response),
        resolved_hash=(
            TreeHash(_string(value["resolved_hash"], response))
            if value.get("resolved_hash") is not None
            else None
        ),
        size=(
            _integer(value["size"], response) if value.get("size") is not None else None
        ),
    )


def _alias_batch(
    response: ApiResponse, key: IdempotencyKey
) -> tuple[AliasReceipt, ...]:
    if response.status not in (200, 201):
        _raise(response, key)
    value = _json_object(response)
    mutation = _mutation(response, key)
    return tuple(
        _alias_batch_entry(item, mutation, response)
        for item in _array(value["aliases"], response)
    )


def _alias_batch_entry(
    value: object,
    mutation: MutationReceipt,
    response: ApiResponse,
) -> AliasReceipt:
    entry = _exact_object(
        value,
        ("path", "target", "target_kind", "dangling", "resolved_hash", "size"),
        response,
    )
    return AliasReceipt(
        mutation=mutation,
        path=_string(entry["path"], response),
        target=_string(entry["target"], response),
        target_kind=(
            _enum_value(AliasTargetKind, entry["target_kind"], response)
            if entry["target_kind"] is not None
            else None
        ),
        dangling=_boolean(entry["dangling"], response),
        resolved_hash=(
            TreeHash(_string(entry["resolved_hash"], response))
            if entry["resolved_hash"] is not None
            else None
        ),
        size=(_integer(entry["size"], response) if entry["size"] is not None else None),
    )


def _options(
    options: RequestOptions, *, key: str | None = None
) -> list[tuple[str, str]]:
    headers = list(options.headers)
    if options.token:
        headers.append(("Authorization", f"Bearer {options.token}"))
    match options:
        case MutationOptions(if_match=if_match) if if_match:
            headers.append(("If-Match", if_match))
        case _:
            pass
    if key:
        headers.append(("Idempotency-Key", key))
    return headers


def _publish_headers(
    options: PublishOptions,
    key: IdempotencyKey,
    creator_claim: CreatorClaim | None,
) -> list[tuple[str, str]]:
    headers = _options(options, key=key)
    headers.append(("Content-Type", str(options.media_type)))
    if options.filename is not None:
        headers.append(
            ("Content-Disposition", f'attachment; filename="{options.filename}"')
        )
    if options.unpack:
        headers.append(("Unpack", "1"))
    if options.managed:
        headers.append(("Management-Action", "claim"))
    if creator_claim is not None:
        headers.append(("Creator-Claim", creator_claim))
    return headers


def _retry_delay(
    policy: RetryPolicy,
    retry_index: int,
    response: ApiResponse | None,
) -> float:
    if policy.honor_retry_after and response is not None:
        retry_after = response.header("Retry-After")
        if retry_after is not None:
            if retry_after.isascii() and retry_after.isdecimal():
                return min(policy.maximum_delay, float(retry_after))
            try:
                parsed = email.utils.parsedate_to_datetime(retry_after)
            except TypeError, ValueError:
                parsed = None
            if parsed is not None:
                return min(
                    policy.maximum_delay,
                    max(
                        0.0,
                        (parsed.astimezone(UTC) - datetime.now(UTC)).total_seconds(),
                    ),
                )
    delay = min(
        policy.maximum_delay,
        policy.initial_delay * policy.backoff_factor**retry_index,
    )
    if policy.jitter is RetryJitter.FULL:
        return secrets.randbelow(max(1, int(delay * 1000) + 1)) / 1000
    return delay


def _expiry_headers(expiry: Expiry) -> list[tuple[str, str]]:
    match expiry:
        case RelativeExpiry(duration=duration):
            return [("Expiry-Mode", "relative"), ("Expiry-In", duration)]
        case AbsoluteExpiry(at=at):
            match at:
                case datetime():
                    encoded = at.isoformat()
                case str():
                    encoded = at
            return [("Expiry-Mode", "absolute"), ("Expiry-At", encoded)]
        case DecayExpiry(
            min_age=min_age,
            max_age=max_age,
            max_size=max_size,
            power=power,
        ):
            return [
                ("Expiry-Mode", "decay"),
                ("Expiry-Min-Age", min_age),
                ("Expiry-Max-Age", max_age),
                ("Expiry-Max-Size", str(max_size)),
                ("Expiry-Power", str(power)),
            ]
        case NeverExpiry():
            return [("Expiry-Mode", "never")]


class _SymbolSync:
    def __init__(
        self,
        client: HttpClient,
        origin: str,
        token: str | None,
        creator_claim: CreatorClaim | None,
        retry_policy: RetryPolicy,
    ) -> None:
        self.http = client
        self.origin = origin.rstrip("/")
        self.token = token
        self.creator_claim = creator_claim
        self.retry_policy = retry_policy
        self.observed_identity: ApiIdentity | None = None

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    def _send(
        self,
        method: HttpMethod | str,
        url: str,
        *,
        headers: Iterable[tuple[str, str]] = (),
        body: Source | None = None,
        timeout: float | None = None,
        _policy: RetryPolicy | None = None,
        _previous_attempts: tuple[RequestAttempt, ...] = (),
    ) -> ApiResponse:
        request = ApiRequest(method, url, tuple(headers), body, timeout)
        policy = self.retry_policy if _policy is None else _policy
        replayable = _source_replayable(body)
        history = list(_previous_attempts)
        attempts = max(1, policy.max_attempts)
        for attempt in range(attempts):
            response: ApiResponse | None = None
            started_at = datetime.now(UTC)
            try:
                response = self.http.request(request)
            except NetworkRequestError as error:
                history.append(
                    RequestAttempt(
                        number=len(history) + 1,
                        started_at=started_at,
                        finished_at=datetime.now(UTC),
                        status=None,
                        error=error,
                    )
                )
                if (
                    not policy.retry_network_errors
                    or not replayable
                    or attempt + 1 == attempts
                ):
                    error.attempts = tuple(history)
                    error.idempotency_key = _header(tuple(headers), "Idempotency-Key")
                    error.replayable = replayable

                    def retry_network(
                        selected: RetryPolicy | None,
                    ) -> ApiResponse:
                        return self._send(
                            method,
                            url,
                            headers=headers,
                            body=body,
                            timeout=timeout,
                            _policy=selected or RetryPolicies.DEFAULT,
                            _previous_attempts=tuple(history),
                        )

                    error._retry = retry_network
                    raise
            else:
                history.append(
                    RequestAttempt(
                        number=len(history) + 1,
                        started_at=started_at,
                        finished_at=datetime.now(UTC),
                        status=response.status,
                        error=None,
                    )
                )
                self.observed_identity = _observe_identity(
                    self.observed_identity,
                    _response_identity(response),
                    response,
                )
                if (
                    response.status not in policy.retry_statuses
                    or not replayable
                    or attempt + 1 == attempts
                ):
                    response.attempts = tuple(history)
                    response.replayable = replayable

                    if response.status >= 400:

                        def retry(selected: RetryPolicy | None) -> ApiResponse:
                            return self._send(
                                method,
                                url,
                                headers=headers,
                                body=body,
                                timeout=timeout,
                                _policy=selected or RetryPolicies.DEFAULT,
                                _previous_attempts=tuple(history),
                            )

                        response._retry = retry
                    return response
                response.close()
            time.sleep(_retry_delay(policy, attempt, response))
        raise RuntimeError("unreachable retry loop")

    def request(self, request: ApiRequest) -> ApiResponse:
        return self._send(
            request.method,
            request.url
            if "://" in request.url
            else self.origin + "/" + request.url.lstrip("/"),
            headers=request.headers,
            body=request.body,
            timeout=request.timeout,
        )

    def stats(self) -> SymbolStats:
        operation = _SyncRetryableOperation(
            lambda: self._send(HttpMethod.GET, self.origin + "/STATS"),
            _decode_symbol_stats,
        )
        return operation._run()

    def api_client(
        self,
        asset: ApiClientAsset,
        options: RequestOptions = RequestOptions(),
    ) -> ApiResponse:
        response = self._send(
            HttpMethod.GET,
            f"{self.origin}/{asset.value}",
            headers=_options(options),
        )
        if response.status not in (200, 304):
            _raise(response)
        return response

    def api_client_hash(
        self,
        asset: ApiClientAsset,
        options: RequestOptions = RequestOptions(),
    ) -> str:
        response = self._send(
            HttpMethod.GET,
            f"{self.origin}/{asset.value}/HASH",
            headers=_options(options),
        )
        return _raw_hash(response)

    def api_manual(
        self,
        manual: ApiManual = ApiManual.INDEX,
        options: RequestOptions = RequestOptions(),
    ) -> ApiResponse:
        response = self._send(
            HttpMethod.GET,
            self.origin + _api_manual_path(manual),
            headers=_options(options),
        )
        if response.status not in (200, 304):
            _raise(response)
        return response

    def api_version(self, options: RequestOptions = RequestOptions()) -> ApiVersionDocument:
        response = self._send(
            HttpMethod.GET,
            self.origin + "/API/VERSION",
            headers=_options(options),
        )
        return _api_version_document(response)

    def create(
        self,
        body: Source,
        options: PublishOptions = PublishOptions(),
    ) -> MutationReceipt:
        key = options.idempotency_key or secrets.token_hex(16)
        return _run_typed_sync(
            lambda: self._send(
                HttpMethod.PUT,
                self.origin + "/",
                headers=_publish_headers(options, key, self.creator_claim),
                body=body,
            ),
            lambda response: _mutation(response, key),
        )

    def sites(self) -> DirectoryListing:
        response = self._send(
            HttpMethod.GET,
            self.origin + "/FILES",
            headers=(("Accept", "application/json"),),
        )
        return _directory(response)

    def site(self, name: str, token: str | None = None) -> SiteClient:
        return SiteClient(self, name, token if token is not None else self.token)

    def close(self) -> None:
        self.http.close()


class SiteClient:
    def __init__(self, symbol: _SymbolSync, name: str, token: str | None) -> None:
        self.symbol, self.name, self.token = symbol, name, token

    @property
    def url(self) -> str:
        return _url(self.symbol.origin, self.name, trailing=True)

    def file(self, path: str) -> FileClient:
        return FileClient(self, path)

    def folder(self, path: str = "") -> FolderClient:
        return FolderClient(self, path)

    def files(self) -> FileInventory:
        response = self.symbol._send(
            HttpMethod.GET,
            _url(self.symbol.origin, self.name, "FILES"),
            headers=(("Accept", "application/json"),),
        )
        return _file_inventory(response)

    def archive(self, format: str = "tar.gz") -> ApiResponse:
        return self.symbol._send(
            HttpMethod.GET,
            f"{self.symbol.origin}/{quote(self.name, safe='')}.{format}",
        )

    def pop(
        self,
        format: str = "tar.gz",
        options: MutationOptions = MutationOptions(),
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.token)
        response = self.symbol._send(
            HttpMethod.DELETE,
            f"{self.symbol.origin}/{quote(self.name, safe='')}.{format}",
            headers=_options(options),
        )
        if response.status != 200:
            _raise(response)
        return response

    def undo(
        self,
        undo_token: str | None = None,
        options: MutationOptions = MutationOptions(),
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.token)
        headers = _options(options)
        if undo_token:
            headers.append(("Undo-Token", undo_token))
        response = self.symbol._send(HttpMethod.UNDO, self.url, headers=headers)
        if response.status != 200:
            _raise(response)
        return response

    def undo_stack(self) -> UndoStack:
        return _undo_stack(
            self.symbol._send(
                HttpMethod.GET,
                _url(self.symbol.origin, self.name, "UNDO"),
            )
        )

    def expiry(self, path: str | None = None) -> ExpirySiteReport | ExpiryReport:
        segments = (
            (self.name, "EXPIRES")
            if path is None
            else (
                self.name,
                *path.split("/"),
                "EXPIRES",
            )
        )
        response = self.symbol._send(
            HttpMethod.GET, _url(self.symbol.origin, *segments)
        )
        if response.status != 200:
            _raise(response)
        value = _json_object(response)
        if path is None:
            return ExpirySiteReport(
                site=_string(value["site"], response),
                entries=tuple(
                    _expiry_report(item, response)
                    for item in _array(value["entries"], response)
                ),
            )
        return _expiry_report(value, response)

    def set_expiry(
        self,
        expiry: Expiry,
        path: str | None = None,
        options: MutationOptions = MutationOptions(),
    ) -> ExpiryReport:
        options = replace(options, token=options.token or self.token)
        target = (
            self.url
            if path is None
            else _url(self.symbol.origin, self.name, *path.split("/"))
        )
        response = self.symbol._send(
            HttpMethod.EXPIRE,
            target,
            headers=(*_options(options), *_expiry_headers(expiry)),
        )
        if response.status != 200:
            _raise(response)
        return _expiry_report(_json_object(response), response)

    def alias(
        self,
        path: str,
        target: str,
        options: MutationOptions = MutationOptions(),
    ) -> AliasReceipt:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Alias-Target", target))
        return _run_typed_sync(
            lambda: self.symbol._send(
                HttpMethod.ALIAS,
                _url(self.symbol.origin, self.name, *path.split("/")),
                headers=headers,
            ),
            lambda response: _alias(response, key),
        )

    def aliases(
        self,
        definitions: Iterable[AliasDefinition],
        options: MutationOptions = MutationOptions(),
    ) -> tuple[AliasReceipt, ...]:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Content-Type", "application/json"))
        body = _json.dumps(
            {
                "aliases": [
                    {"path": item.path, "target": item.target} for item in definitions
                ]
            },
            separators=(",", ":"),
        )
        return _run_typed_sync(
            lambda: self.symbol._send(
                HttpMethod.ALIAS, self.url, headers=headers, body=body
            ),
            lambda response: _alias_batch(response, key),
        )

    def copy(
        self, destination: str, options: MutationOptions = MutationOptions()
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Destination", "/" + destination.strip("/")))
        return _run_typed_sync(
            lambda: self.symbol._send(HttpMethod.COPY, self.url, headers=headers),
            lambda response: _mutation(response, key, True),
        )

    def move(
        self, destination: str, options: MutationOptions = MutationOptions()
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        headers = _options(options)
        headers.append(("Destination", "/" + destination.strip("/")))
        return _run_typed_sync(
            lambda: self.symbol._send(HttpMethod.MOVE, self.url, headers=headers),
            lambda response: _mutation(response, expected_changed=True),
        )

    def management(self) -> ManagementClient:
        return ManagementClient(self)


class FolderClient:
    def __init__(self, site: SiteClient, path: str) -> None:
        self.site, self.path = site, path.strip("/")

    def create(
        self,
        body: Source,
        options: CreateFileOptions = CreateFileOptions(),
    ) -> AllocationReceipt:
        options = replace(options, token=options.token or self.site.token)
        if callable(options.name):
            return self._custom(body, options, options.name)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Content-Type", str(options.media_type or MediaTypes.BINARY)))
        if options.name.prefix:
            headers.append(("File-Prefix", options.name.prefix))
        if options.name.suffix:
            headers.append(("File-Suffix", options.name.suffix))
        if options.name.extension:
            headers.append(("File-Extension", options.name.extension.lstrip(".")))
        url = _url(
            self.site.symbol.origin,
            self.site.name,
            *self.path.split("/"),
            trailing=True,
        )
        return _run_typed_sync(
            lambda: self.site.symbol._send(
                HttpMethod.POST,
                url,
                headers=headers,
                body=body,
            ),
            lambda response: _allocation(response, key),
        )

    def _custom(
        self,
        body: Source,
        options: CreateFileOptions,
        naming: Callable[[ProposedFileName], str | Awaitable[str]],
    ) -> AllocationReceipt:
        logical_key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=logical_key + ":proposal")
        headers.extend(
            (
                ("Allocation-Action", "propose"),
                ("Content-Type", str(options.media_type or MediaTypes.BINARY)),
            )
        )
        url = _url(
            self.site.symbol.origin,
            self.site.name,
            *self.path.split("/"),
            trailing=True,
        )
        proposal_response = self.site.symbol._send(
            HttpMethod.POST, url, headers=headers, body=body
        )
        if proposal_response.status not in (200, 202):
            _raise(proposal_response, logical_key)
        value = _exact_object(
            _json_object(proposal_response),
            (
                "allocation_token",
                "expires_at",
                "proposal",
                "idempotency_key",
                "replayed",
            ),
            proposal_response,
        )
        details = _exact_object(
            value["proposal"],
            (
                "folder",
                "default_name",
                "hash",
                "size",
                "media_type",
                "inferred_extension",
            ),
            proposal_response,
        )
        proposal = ProposedFileName(
            folder=_string(details["folder"], proposal_response),
            default_name=_string(details["default_name"], proposal_response),
            hash=TreeHash(_string(details["hash"], proposal_response)),
            size=_integer(details["size"], proposal_response),
            token=_string(value["allocation_token"], proposal_response),
        )
        try:
            filename = naming(proposal)
            if inspect.isawaitable(filename):
                if inspect.iscoroutine(filename):
                    filename.close()
                raise TypeError("sync allocation naming callback returned an awaitable")
        except BaseException:
            cancel_headers = _options(options, key=logical_key + ":cancel")
            cancel_headers.extend(
                (
                    ("Allocation-Action", "cancel"),
                    ("Allocation-Token", proposal.token),
                )
            )
            self.site.symbol._send(HttpMethod.POST, url, headers=cancel_headers)
            raise
        final_headers = _options(options, key=logical_key + ":finalize")
        final_headers.extend(
            (
                ("Allocation-Action", "finalize"),
                ("Allocation-Token", proposal.token),
                ("File-Name", filename),
            )
        )
        return _run_typed_sync(
            lambda: self.site.symbol._send(HttpMethod.POST, url, headers=final_headers),
            lambda response: _allocation(response, logical_key),
        )

    def bytes(
        self, body: ByteSource, options: CreateFileOptions = CreateFileOptions()
    ) -> AllocationReceipt:
        return self.create(body, options)

    def text(
        self, body: str, options: CreateFileOptions = CreateFileOptions()
    ) -> AllocationReceipt:
        return self.create(
            body, replace(options, media_type=options.media_type or MediaTypes.TEXT)
        )

    def json(
        self, value: Any, options: CreateFileOptions = CreateFileOptions()
    ) -> AllocationReceipt:
        return self.create(
            _json.dumps(value, separators=(",", ":")),
            replace(options, media_type=options.media_type or MediaTypes.JSON),
        )


class FileClient:
    def __init__(self, site: SiteClient, path: str) -> None:
        self.site, self.path = site, path.strip("/")

    @property
    def url(self) -> str:
        return _url(self.site.symbol.origin, self.site.name, *self.path.split("/"))

    def get(self, options: RequestOptions = RequestOptions()) -> ApiResponse:
        return self.site.symbol._send(
            HttpMethod.GET,
            self.url,
            headers=_options(replace(options, token=options.token or self.site.token)),
        )

    def text(self) -> str:
        response = self.get()
        if response.status != 200:
            _raise(response)
        return response.text()

    def bytes(self) -> bytes:
        response = self.get()
        if response.status != 200:
            _raise(response)
        return response.body

    def json(self) -> Any:
        return _json.loads(self.text())

    def put(
        self,
        body: Source,
        media_type: MediaType | str = MediaTypes.BINARY,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        headers = _options(options)
        headers.append(("Content-Type", str(media_type)))
        return _run_typed_sync(
            lambda: self.site.symbol._send(
                HttpMethod.PUT, self.url, headers=headers, body=body
            ),
            _mutation,
        )

    def remove(self, options: MutationOptions = MutationOptions()) -> DeleteReceipt:
        options = replace(options, token=options.token or self.site.token)
        return _run_typed_sync(
            lambda: self.site.symbol._send(
                HttpMethod.DELETE, self.url, headers=_options(options)
            ),
            _delete_receipt,
        )

    def hash(self) -> Blake3:
        response = self.site.symbol._send(HttpMethod.GET, self.url + "/HASH")
        return _raw_hash(response)

    def replace(
        self,
        body: Source,
        *,
        base_hash: str,
        media_type: MediaType | str = MediaTypes.BINARY,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.extend(
            (
                ("Content-Type", str(media_type)),
                ("If-Content-Match", f'"{base_hash}"'),
            )
        )
        return _run_typed_sync(
            lambda: self.site.symbol._send(
                HttpMethod.REPLACE, self.url, headers=headers, body=body
            ),
            lambda response: _mutation(response, key),
        )

    def splice(
        self,
        change: ByteSplice,
        *,
        base_hash: str,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        return self.patch((change,), base_hash=base_hash, options=options)

    def patch(
        self,
        changes: Iterable[ByteSplice],
        *,
        base_hash: str,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        changes = tuple(changes)
        insertions = tuple(_body(item.insert) or b"" for item in changes)
        descriptor = ",".join(
            f"offset={item.offset}; delete={item.delete_bytes}; insert={len(insertion)}"
            for item, insertion in zip(changes, insertions, strict=True)
        )
        headers = _options(options, key=key)
        headers.append(("If-Content-Match", f'"{base_hash}"'))
        if len(changes) <= 64 and len(descriptor.encode()) <= 8192:
            headers.append(("Splice", descriptor))
            body = b"".join(insertions)
        else:
            headers.append(("Content-Type", "application/vnd.symbol.splice; version=1"))
            frame = bytearray(b"SYMSPL1\0")
            frame.extend(len(changes).to_bytes(4, "big"))
            frame.extend((0).to_bytes(4, "big"))
            for item, insertion in zip(changes, insertions, strict=True):
                frame.extend(item.offset.to_bytes(8, "big"))
                frame.extend(item.delete_bytes.to_bytes(8, "big"))
                frame.extend(len(insertion).to_bytes(8, "big"))
            frame.extend(b"".join(insertions))
            body = bytes(frame)
        return _run_typed_sync(
            lambda: self.site.symbol._send(
                HttpMethod.PATCH, self.url, headers=headers, body=body
            ),
            lambda response: _mutation(response, key),
        )


class ManagementClient:
    def __init__(self, site: SiteClient) -> None:
        self.site = site

    def action(
        self, action: ManagementAction, options: MutationOptions = MutationOptions()
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Management-Action", action.value))
        if (
            action in (ManagementAction.CLAIM, ManagementAction.ROTATE)
            and self.site.symbol.creator_claim is not None
        ):
            headers.append(("Creator-Claim", self.site.symbol.creator_claim))
        response = self.site.symbol._send(
            HttpMethod.MANAGE, self.site.url, headers=headers
        )
        if response.status != 200:
            _raise(response, key)
        return response

    def status(self) -> ManagementStatus:
        return _management_status(self.action(ManagementAction.STATUS))

    def claim(self) -> tuple[ManagementStatus, str]:
        response = self.action(ManagementAction.CLAIM)
        token = response.header("Management-Token")
        if token is None:
            raise UnexpectedResponseError(response)
        return ManagementStatus(True), ManagementToken(token)

    def rotate(self) -> tuple[ManagementStatus, str]:
        response = self.action(ManagementAction.ROTATE)
        token = response.header("Management-Token")
        if token is None:
            raise UnexpectedResponseError(response)
        return ManagementStatus(True), ManagementToken(token)

    def release(self) -> ManagementStatus:
        return _management_status(self.action(ManagementAction.RELEASE))


class _SymbolAsync:
    def __init__(
        self,
        client: AsyncHttpClient,
        origin: str,
        token: ManagementToken | None,
        creator_claim: CreatorClaim | None,
        retry_policy: RetryPolicy,
    ) -> None:
        self.http = client
        self.origin = origin.rstrip("/")
        self.token = token
        self.creator_claim = creator_claim
        self.retry_policy = retry_policy
        self.observed_identity: ApiIdentity | None = None

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.close()

    async def _send(
        self,
        method: HttpMethod | str,
        url: str,
        *,
        headers: Iterable[tuple[str, str]] = (),
        body: AsyncSource | None = None,
        _policy: RetryPolicy | None = None,
        _previous_attempts: tuple[RequestAttempt, ...] = (),
    ) -> ApiResponse:
        request = AsyncApiRequest(method, url, tuple(headers), body)
        policy = self.retry_policy if _policy is None else _policy
        replayable = _source_replayable(body)
        history = list(_previous_attempts)
        attempts = max(1, policy.max_attempts)
        for attempt in range(attempts):
            response: ApiResponse | None = None
            started_at = datetime.now(UTC)
            try:
                response = await self.http.request(request)
            except NetworkRequestError as error:
                history.append(
                    RequestAttempt(
                        number=len(history) + 1,
                        started_at=started_at,
                        finished_at=datetime.now(UTC),
                        status=None,
                        error=error,
                    )
                )
                if (
                    not policy.retry_network_errors
                    or not replayable
                    or attempt + 1 == attempts
                ):
                    error.attempts = tuple(history)
                    error.idempotency_key = _header(tuple(headers), "Idempotency-Key")
                    error.replayable = replayable

                    async def retry_network(
                        selected: RetryPolicy | None,
                    ) -> ApiResponse:
                        return await self._send(
                            method,
                            url,
                            headers=headers,
                            body=body,
                            _policy=selected or RetryPolicies.DEFAULT,
                            _previous_attempts=tuple(history),
                        )

                    error._async_retry = retry_network
                    raise
            else:
                history.append(
                    RequestAttempt(
                        number=len(history) + 1,
                        started_at=started_at,
                        finished_at=datetime.now(UTC),
                        status=response.status,
                        error=None,
                    )
                )
                content_type = response.header("Content-Type") or ""
                if (
                    response.status >= 400
                    or str(method) not in (HttpMethod.GET, HttpMethod.HEAD)
                    or content_type.casefold().startswith("application/json")
                    or response.header("Content-Revision") is not None
                ):
                    await response.aread()
                self.observed_identity = _observe_identity(
                    self.observed_identity,
                    _response_identity(response),
                    response,
                )
                if (
                    response.status not in policy.retry_statuses
                    or not replayable
                    or attempt + 1 == attempts
                ):
                    response.attempts = tuple(history)
                    response.replayable = replayable

                    if response.status >= 400:

                        async def retry(
                            selected: RetryPolicy | None,
                        ) -> ApiResponse:
                            return await self._send(
                                method,
                                url,
                                headers=headers,
                                body=body,
                                _policy=selected or RetryPolicies.DEFAULT,
                                _previous_attempts=tuple(history),
                            )

                        response._async_retry = retry
                    return response
                await response.aclose()
            await asyncio.sleep(_retry_delay(policy, attempt, response))
        raise RuntimeError("unreachable retry loop")

    async def request(self, request: AsyncApiRequest) -> ApiResponse:
        return await self._send(
            request.method,
            request.url
            if "://" in request.url
            else self.origin + "/" + request.url.lstrip("/"),
            headers=request.headers,
            body=request.body,
        )

    async def stats(self) -> SymbolStats:
        operation = _AsyncRetryableOperation(
            lambda: self._send(HttpMethod.GET, self.origin + "/STATS"),
            _decode_symbol_stats_async,
        )
        return await operation._run()

    async def api_client(
        self,
        asset: ApiClientAsset,
        options: RequestOptions = RequestOptions(),
    ) -> ApiResponse:
        response = await self._send(
            HttpMethod.GET,
            f"{self.origin}/{asset.value}",
            headers=_options(options),
        )
        if response.status not in (200, 304):
            _raise(response)
        return response

    async def api_client_hash(
        self,
        asset: ApiClientAsset,
        options: RequestOptions = RequestOptions(),
    ) -> str:
        response = await self._send(
            HttpMethod.GET,
            f"{self.origin}/{asset.value}/HASH",
            headers=_options(options),
        )
        if response.status != 200:
            _raise(response)
        try:
            return Blake3((await response.atext()).strip())
        except ValueError as error:
            raise MalformedResponseError(response) from error

    async def api_manual(
        self,
        manual: ApiManual = ApiManual.INDEX,
        options: RequestOptions = RequestOptions(),
    ) -> ApiResponse:
        response = await self._send(
            HttpMethod.GET,
            self.origin + _api_manual_path(manual),
            headers=_options(options),
        )
        if response.status not in (200, 304):
            _raise(response)
        return response

    async def api_version(
        self, options: RequestOptions = RequestOptions()
    ) -> ApiVersionDocument:
        response = await self._send(
            HttpMethod.GET,
            self.origin + "/API/VERSION",
            headers=_options(options),
        )
        return _api_version_document(response)

    async def create(
        self,
        body: AsyncSource,
        options: PublishOptions = PublishOptions(),
    ) -> MutationReceipt:
        key = options.idempotency_key or secrets.token_hex(16)
        return await _run_typed_async(
            lambda: self._send(
                HttpMethod.PUT,
                self.origin + "/",
                headers=_publish_headers(options, key, self.creator_claim),
                body=body,
            ),
            lambda response: _mutation(response, key),
        )

    async def sites(self) -> DirectoryListing:
        response = await self._send(
            HttpMethod.GET,
            self.origin + "/FILES",
            headers=(("Accept", "application/json"),),
        )
        return _directory(response)

    async def close(self) -> None:
        await self.http.close()

    def site(self, name: str, token: str | None = None) -> AsyncSiteClient:
        return AsyncSiteClient(self, name, token if token is not None else self.token)


class AsyncSiteClient:
    def __init__(self, symbol: _SymbolAsync, name: str, token: str | None) -> None:
        self.symbol, self.name, self.token = symbol, name, token

    @property
    def url(self) -> str:
        return _url(self.symbol.origin, self.name, trailing=True)

    def file(self, path: str) -> AsyncFileClient:
        return AsyncFileClient(self, path)

    def folder(self, path: str = "") -> AsyncFolderClient:
        return AsyncFolderClient(self, path)

    async def files(self) -> FileInventory:
        response = await self.symbol._send(
            HttpMethod.GET,
            _url(self.symbol.origin, self.name, "FILES"),
            headers=(("Accept", "application/json"),),
        )
        return _file_inventory(response)

    async def alias(
        self,
        path: str,
        target: str,
        options: MutationOptions = MutationOptions(),
    ) -> AliasReceipt:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Alias-Target", target))
        return await _run_typed_async(
            lambda: self.symbol._send(
                HttpMethod.ALIAS,
                _url(self.symbol.origin, self.name, *path.split("/")),
                headers=headers,
            ),
            lambda response: _alias(response, key),
        )

    async def aliases(
        self,
        definitions: Iterable[AliasDefinition],
        options: MutationOptions = MutationOptions(),
    ) -> tuple[AliasReceipt, ...]:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Content-Type", "application/json"))
        body = _json.dumps(
            {
                "aliases": [
                    {"path": item.path, "target": item.target} for item in definitions
                ]
            },
            separators=(",", ":"),
        )
        return await _run_typed_async(
            lambda: self.symbol._send(
                HttpMethod.ALIAS, self.url, headers=headers, body=body
            ),
            lambda response: _alias_batch(response, key),
        )

    async def archive(self, format: str = "tar.gz") -> ApiResponse:
        return await self.symbol._send(
            HttpMethod.GET,
            f"{self.symbol.origin}/{quote(self.name, safe='')}.{format}",
        )

    async def pop(
        self,
        format: str = "tar.gz",
        options: MutationOptions = MutationOptions(),
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.token)
        response = await self.symbol._send(
            HttpMethod.DELETE,
            f"{self.symbol.origin}/{quote(self.name, safe='')}.{format}",
            headers=_options(options),
        )
        if response.status != 200:
            _raise(response)
        return response

    async def copy(
        self,
        destination: SiteName,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Destination", "/" + destination.strip("/")))
        return await _run_typed_async(
            lambda: self.symbol._send(HttpMethod.COPY, self.url, headers=headers),
            lambda response: _mutation(response, key, True),
        )

    async def move(
        self,
        destination: SiteName,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        headers = _options(options)
        headers.append(("Destination", "/" + destination.strip("/")))
        return await _run_typed_async(
            lambda: self.symbol._send(HttpMethod.MOVE, self.url, headers=headers),
            lambda response: _mutation(response, expected_changed=True),
        )

    async def undo(
        self,
        undo_token: UndoToken | None = None,
        options: MutationOptions = MutationOptions(),
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.token)
        headers = _options(options)
        if undo_token is not None:
            headers.append(("Undo-Token", undo_token))
        response = await self.symbol._send(HttpMethod.UNDO, self.url, headers=headers)
        if response.status != 200:
            _raise(response)
        return response

    async def undo_stack(self) -> UndoStack:
        return _undo_stack(
            await self.symbol._send(
                HttpMethod.GET,
                _url(self.symbol.origin, self.name, "UNDO"),
            )
        )

    async def expiry(
        self, path: SitePath | None = None
    ) -> ExpirySiteReport | ExpiryReport:
        segments = (
            (self.name, "EXPIRES")
            if path is None
            else (self.name, *path.split("/"), "EXPIRES")
        )
        response = await self.symbol._send(
            HttpMethod.GET, _url(self.symbol.origin, *segments)
        )
        if response.status != 200:
            _raise(response)
        value = _json_object(response)
        if path is None:
            return ExpirySiteReport(
                site=_string(value["site"], response),
                entries=tuple(
                    _expiry_report(item, response)
                    for item in _array(value["entries"], response)
                ),
            )
        return _expiry_report(value, response)

    async def set_expiry(
        self,
        expiry: Expiry,
        path: SitePath | None = None,
        options: MutationOptions = MutationOptions(),
    ) -> ExpiryReport:
        options = replace(options, token=options.token or self.token)
        target = (
            self.url
            if path is None
            else _url(self.symbol.origin, self.name, *path.split("/"))
        )
        response = await self.symbol._send(
            HttpMethod.EXPIRE,
            target,
            headers=(*_options(options), *_expiry_headers(expiry)),
        )
        if response.status != 200:
            _raise(response)
        return _expiry_report(_json_object(response), response)

    def management(self) -> AsyncManagementClient:
        return AsyncManagementClient(self)


class AsyncFolderClient:
    def __init__(self, site: AsyncSiteClient, path: str) -> None:
        self.site, self.path = site, path.strip("/")

    async def create(
        self,
        body: AsyncSource,
        options: CreateFileOptions = CreateFileOptions(),
    ) -> AllocationReceipt:
        options = replace(options, token=options.token or self.site.token)
        name = options.name
        if callable(name):
            return await self._custom(body, options, name)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Content-Type", str(options.media_type or MediaTypes.BINARY)))
        if name.prefix:
            headers.append(("File-Prefix", name.prefix))
        if name.suffix:
            headers.append(("File-Suffix", name.suffix))
        if name.extension:
            headers.append(("File-Extension", name.extension.lstrip(".")))
        url = _url(
            self.site.symbol.origin,
            self.site.name,
            *self.path.split("/"),
            trailing=True,
        )
        return await _run_typed_async(
            lambda: self.site.symbol._send(
                HttpMethod.POST,
                url,
                headers=headers,
                body=body,
            ),
            lambda response: _allocation(response, key),
        )

    async def _custom(
        self,
        body: AsyncSource,
        options: CreateFileOptions,
        naming: Callable[[ProposedFileName], str | Awaitable[str]],
    ) -> AllocationReceipt:
        logical_key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=logical_key + ":proposal")
        headers.extend(
            (
                ("Allocation-Action", "propose"),
                ("Content-Type", str(options.media_type or MediaTypes.BINARY)),
            )
        )
        url = _url(
            self.site.symbol.origin,
            self.site.name,
            *self.path.split("/"),
            trailing=True,
        )
        proposal_response = await self.site.symbol._send(
            HttpMethod.POST, url, headers=headers, body=body
        )
        if proposal_response.status not in (200, 202):
            _raise(proposal_response, logical_key)
        value = _exact_object(
            _json_object(proposal_response),
            (
                "allocation_token",
                "expires_at",
                "proposal",
                "idempotency_key",
                "replayed",
            ),
            proposal_response,
        )
        details = _exact_object(
            value["proposal"],
            (
                "folder",
                "default_name",
                "hash",
                "size",
                "media_type",
                "inferred_extension",
            ),
            proposal_response,
        )
        proposal = ProposedFileName(
            folder=_string(details["folder"], proposal_response),
            default_name=_string(details["default_name"], proposal_response),
            hash=TreeHash(_string(details["hash"], proposal_response)),
            size=_integer(details["size"], proposal_response),
            token=_string(value["allocation_token"], proposal_response),
        )
        try:
            selected = naming(proposal)
            filename = await selected if inspect.isawaitable(selected) else selected
        except BaseException:
            cancel_headers = _options(options, key=logical_key + ":cancel")
            cancel_headers.extend(
                (
                    ("Allocation-Action", "cancel"),
                    ("Allocation-Token", proposal.token),
                )
            )
            await self.site.symbol._send(HttpMethod.POST, url, headers=cancel_headers)
            raise
        final_headers = _options(options, key=logical_key + ":finalize")
        final_headers.extend(
            (
                ("Allocation-Action", "finalize"),
                ("Allocation-Token", proposal.token),
                ("File-Name", filename),
            )
        )
        return await _run_typed_async(
            lambda: self.site.symbol._send(HttpMethod.POST, url, headers=final_headers),
            lambda response: _allocation(response, logical_key),
        )

    async def bytes(
        self,
        body: AsyncByteSource,
        options: CreateFileOptions = CreateFileOptions(),
    ) -> AllocationReceipt:
        return await self.create(body, options)

    async def text(
        self,
        body: str,
        options: CreateFileOptions = CreateFileOptions(),
    ) -> AllocationReceipt:
        return await self.create(
            body,
            replace(options, media_type=options.media_type or MediaTypes.TEXT),
        )

    async def json(
        self,
        value: Any,
        options: CreateFileOptions = CreateFileOptions(),
    ) -> AllocationReceipt:
        return await self.create(
            _json.dumps(value, separators=(",", ":")),
            replace(options, media_type=options.media_type or MediaTypes.JSON),
        )


class AsyncFileClient:
    def __init__(self, site: AsyncSiteClient, path: str) -> None:
        self.site, self.path = site, path.strip("/")

    @property
    def url(self) -> str:
        return _url(self.site.symbol.origin, self.site.name, *self.path.split("/"))

    async def get(self, options: RequestOptions = RequestOptions()) -> ApiResponse:
        options = replace(options, token=options.token or self.site.token)
        return await self.site.symbol._send(
            HttpMethod.GET, self.url, headers=_options(options)
        )

    async def text(self) -> str:
        response = await self.get()
        if response.status != 200:
            _raise(response)
        return await response.atext()

    async def bytes(self) -> bytes:
        response = await self.get()
        if response.status != 200:
            _raise(response)
        return await response.aread()

    async def json(self) -> Any:
        return _json.loads(await self.text())

    async def put(
        self,
        body: AsyncSource,
        media_type: MediaType | str = MediaTypes.BINARY,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        headers = _options(options)
        headers.append(("Content-Type", str(media_type)))
        return await _run_typed_async(
            lambda: self.site.symbol._send(
                HttpMethod.PUT, self.url, headers=headers, body=body
            ),
            _mutation,
        )

    async def remove(
        self, options: MutationOptions = MutationOptions()
    ) -> DeleteReceipt:
        options = replace(options, token=options.token or self.site.token)
        return await _run_typed_async(
            lambda: self.site.symbol._send(
                HttpMethod.DELETE, self.url, headers=_options(options)
            ),
            _delete_receipt,
        )

    async def hash(self) -> Blake3:
        response = await self.site.symbol._send(HttpMethod.GET, self.url + "/HASH")
        if response.status != 200:
            _raise(response)
        try:
            return Blake3((await response.atext()).strip())
        except ValueError as error:
            raise MalformedResponseError(response) from error

    async def replace(
        self,
        body: AsyncSource,
        *,
        base_hash: Blake3,
        media_type: MediaType | str = MediaTypes.BINARY,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.extend(
            (
                ("Content-Type", str(media_type)),
                ("If-Content-Match", f'"{base_hash}"'),
            )
        )
        return await _run_typed_async(
            lambda: self.site.symbol._send(
                HttpMethod.REPLACE, self.url, headers=headers, body=body
            ),
            lambda response: _mutation(response, key),
        )

    async def splice(
        self,
        change: AsyncByteSplice,
        *,
        base_hash: Blake3,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        return await self.patch((change,), base_hash=base_hash, options=options)

    async def patch(
        self,
        changes: Iterable[AsyncByteSplice],
        *,
        base_hash: Blake3,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        changes = tuple(changes)
        insertions = tuple(
            [await _async_splice_insert(item.insert) for item in changes]
        )
        descriptor = ",".join(
            f"offset={item.offset}; delete={item.delete_bytes}; insert={len(insertion)}"
            for item, insertion in zip(changes, insertions, strict=True)
        )
        headers = _options(options, key=key)
        headers.append(("If-Content-Match", f'"{base_hash}"'))
        if len(changes) <= 64 and len(descriptor.encode()) <= 8192:
            headers.append(("Splice", descriptor))
            body = b"".join(insertions)
        else:
            headers.append(("Content-Type", "application/vnd.symbol.splice; version=1"))
            frame = bytearray(b"SYMSPL1\0")
            frame.extend(len(changes).to_bytes(4, "big"))
            frame.extend((0).to_bytes(4, "big"))
            for item, insertion in zip(changes, insertions, strict=True):
                frame.extend(item.offset.to_bytes(8, "big"))
                frame.extend(item.delete_bytes.to_bytes(8, "big"))
                frame.extend(len(insertion).to_bytes(8, "big"))
            frame.extend(b"".join(insertions))
            body = bytes(frame)
        return await _run_typed_async(
            lambda: self.site.symbol._send(
                HttpMethod.PATCH, self.url, headers=headers, body=body
            ),
            lambda response: _mutation(response, key),
        )


class AsyncManagementClient:
    def __init__(self, site: AsyncSiteClient) -> None:
        self.site = site

    async def action(
        self,
        action: ManagementAction,
        options: MutationOptions = MutationOptions(),
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Management-Action", action.value))
        if (
            action in (ManagementAction.CLAIM, ManagementAction.ROTATE)
            and self.site.symbol.creator_claim is not None
        ):
            headers.append(("Creator-Claim", self.site.symbol.creator_claim))
        response = await self.site.symbol._send(
            HttpMethod.MANAGE, self.site.url, headers=headers
        )
        if response.status != 200:
            _raise(response, key)
        return response

    async def status(self) -> ManagementStatus:
        return _management_status(await self.action(ManagementAction.STATUS))

    async def claim(self) -> tuple[ManagementStatus, ManagementToken]:
        response = await self.action(ManagementAction.CLAIM)
        token = response.header("Management-Token")
        if token is None:
            raise UnexpectedResponseError(response)
        return ManagementStatus(True), ManagementToken(token)

    async def rotate(self) -> tuple[ManagementStatus, ManagementToken]:
        response = await self.action(ManagementAction.ROTATE)
        token = response.header("Management-Token")
        if token is None:
            raise UnexpectedResponseError(response)
        return ManagementStatus(True), ManagementToken(token)

    async def release(self) -> ManagementStatus:
        return _management_status(await self.action(ManagementAction.RELEASE))


@overload
def Symbol(
    client: None = None,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    creator_claim: CreatorClaim | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolSync: ...


@overload
def Symbol(
    client: HttpClient,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    creator_claim: CreatorClaim | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolSync: ...


@overload
def Symbol(
    client: AsyncHttpClient,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    creator_claim: CreatorClaim | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolAsync: ...


def Symbol(
    client: HttpClient | AsyncHttpClient | None = None,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    creator_claim: CreatorClaim | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolSync | _SymbolAsync:
    token = ManagementToken(token) if token is not None else None
    creator_claim = CreatorClaim(creator_claim) if creator_claim is not None else None
    match client:
        case None:
            return _SymbolSync(
                HttpClient.stdlib(), origin, token, creator_claim, retry_policy
            )
        case HttpClient():
            return _SymbolSync(client, origin, token, creator_claim, retry_policy)
        case AsyncHttpClient():
            return _SymbolAsync(client, origin, token, creator_claim, retry_policy)


def _directory(response: ApiResponse) -> DirectoryListing:
    if response.status != 200:
        _raise(response)
    value = _exact_object(
        _json_object(response),
        ("path", "files", "aliases", "bytes", "entries"),
        response,
    )
    return DirectoryListing(
        path=_string(value["path"], response),
        files=_integer(value["files"], response),
        aliases=_integer(value["aliases"], response),
        bytes=_integer(value["bytes"], response),
        entries=tuple(
            _directory_entry(item, response)
            for item in _array(value["entries"], response)
        ),
    )


def _directory_entry(value: object, response: ApiResponse) -> DirectoryEntry:
    if not isinstance(value, dict):
        raise MalformedResponseError(response)
    unknown = cast(Mapping[object, object], value)
    kind_value = unknown.get("kind")
    kind = _enum_value(DirectoryKind, kind_value, response)
    match kind:
        case DirectoryKind.BUILTIN | DirectoryKind.SITE | DirectoryKind.DIRECTORY:
            entry = _exact_object(
                cast(object, value),
                ("kind", "name", "files", "bytes"),
                response,
            )
            return DirectoryEntry(
                kind=kind,
                name=_string(entry["name"], response),
                files=(
                    _integer(entry["files"], response)
                    if entry["files"] is not None
                    else None
                ),
                bytes=_integer(entry["bytes"], response),
                target=None,
                target_kind=None,
                dangling=None,
            )
        case DirectoryKind.FILE:
            entry = _exact_object(
                cast(object, value), ("kind", "name", "bytes"), response
            )
            return DirectoryEntry(
                kind=kind,
                name=_string(entry["name"], response),
                files=None,
                bytes=_integer(entry["bytes"], response),
                target=None,
                target_kind=None,
                dangling=None,
            )
        case DirectoryKind.ALIAS:
            entry = _exact_object(
                cast(object, value),
                (
                    "kind",
                    "name",
                    "target",
                    "target_kind",
                    "dangling",
                    "files",
                    "bytes",
                ),
                response,
            )
            return DirectoryEntry(
                kind=kind,
                name=_string(entry["name"], response),
                files=(
                    _integer(entry["files"], response)
                    if entry["files"] is not None
                    else None
                ),
                bytes=(
                    _integer(entry["bytes"], response)
                    if entry["bytes"] is not None
                    else 0
                ),
                target=_string(entry["target"], response),
                target_kind=(
                    _enum_value(AliasTargetKind, entry["target_kind"], response)
                    if entry["target_kind"] is not None
                    else None
                ),
                dangling=_boolean(entry["dangling"], response),
            )


__all__ = (
    "API_REVISION",
    "API_VERSION",
    "API_VERSION_PARTS",
    "BUILD_COMMIT",
    "BUILD_DIRTY",
    "GENERATOR_VERSION",
    "GENERATOR_VERSION_PARTS",
    "METADATA",
    "METADATA_JSON",
    "SOURCE_HASH",
    "AbsoluteExpiry",
    "AliasDefinition",
    "AliasInventoryEntry",
    "AliasReceipt",
    "AliasTargetKind",
    "AllocationReceipt",
    "AbsoluteExpiryPolicy",
    "ApiArtifact",
    "ApiClientAsset",
    "ApiIdentity",
    "ApiManual",
    "ApiVersionDocument",
    "ApiMetadata",
    "ApiRequest",
    "ApiResponse",
    "ApiVersion",
    "AsyncApiRequest",
    "AsyncByteSource",
    "AsyncHttpClient",
    "AsyncFileClient",
    "AsyncFolderClient",
    "AsyncManagementClient",
    "AsyncResponseStream",
    "AsyncSiteClient",
    "AsyncSource",
    "Blake3",
    "BodyNotReplayableError",
    "ByteSplice",
    "CacheStats",
    "Charset",
    "ContentFormat",
    "ContentFormats",
    "CreatorClaim",
    "CreateFileOptions",
    "DecayExpiry",
    "DecayExpiryPolicy",
    "DeleteReceipt",
    "DirectoryEntry",
    "DirectoryKind",
    "DirectoryListing",
    "EntityTag",
    "ExpiryCap",
    "ExpiryLimit",
    "ExpiryKind",
    "ExpiryReport",
    "ExpirySiteReport",
    "ExpiryTarget",
    "FileEntry",
    "FileInventory",
    "GeneratedName",
    "GitCommit",
    "HttpClient",
    "HttpMethod",
    "ManagementStatus",
    "ManagementAction",
    "ManagementClient",
    "MalformedResponseError",
    "ManagementToken",
    "MediaType",
    "MediaTypes",
    "MissingOptionalDependency",
    "MutationOptions",
    "MutationReceipt",
    "NeverExpiry",
    "NetworkRequestError",
    "OwnedExpiryPolicy",
    "OperationStateError",
    "PreconditionFailedError",
    "PublishOptions",
    "RangeNotSatisfiableError",
    "ReaderStats",
    "RelativeExpiry",
    "RelativeExpiryPolicy",
    "RequestOptions",
    "RequestAttempt",
    "RetryPolicies",
    "RetryPolicy",
    "RetryJitter",
    "ServingStats",
    "SizeDistribution",
    "Symbol",
    "SymbolApiError",
    "SymbolStats",
    "SyncResponseStream",
    "TreeHash",
    "UnauthorizedError",
    "UndoEntry",
    "UndoKind",
    "UndoToken",
    "UndoStack",
)

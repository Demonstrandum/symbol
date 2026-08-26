from __future__ import annotations

import http.client
import json as _json
import secrets
import time
from collections.abc import Awaitable, Callable, Iterable, Mapping
from dataclasses import dataclass, replace
from datetime import UTC, datetime
from enum import StrEnum
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


@dataclass(frozen=True, slots=True)
class ApiMetadata:
    artifact: str
    api_version: str
    absolute_revision: int
    source_hash: str
    generator_version: str
    commit: str
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
            return ApiMetadata(
                artifact=artifact,
                api_version=api_version,
                absolute_revision=absolute_revision,
                source_hash=source_hash,
                generator_version=generator_version,
                commit=commit,
                dirty=dirty,
            )
        case _:
            raise ValueError("generated API metadata does not match its schema")


METADATA: Final[ApiMetadata] = _metadata(METADATA_JSON)

API_VERSION: Final[str] = METADATA.api_version
API_REVISION: Final[int] = METADATA.absolute_revision
SOURCE_HASH: Final[str] = METADATA.source_hash
GENERATOR_VERSION: Final[str] = METADATA.generator_version
BUILD_COMMIT: Final[str] = METADATA.commit
BUILD_DIRTY: Final[bool] = METADATA.dirty


class ByteReader(Protocol):
    def read(self, size: int = -1, /) -> bytes: ...


type ByteSource = bytes | bytearray | memoryview[int] | ByteReader | Path
type Source = ByteSource | str
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


@dataclass(frozen=True, slots=True)
class ApiRequest:
    method: HttpMethod | str
    url: str
    headers: Headers = ()
    body: Source | None = None
    timeout: float | None = None


@dataclass(frozen=True, slots=True)
class ApiResponse:
    status: int
    headers: Headers
    body: bytes

    def header(self, name: str) -> str | None:
        return _header(self.headers, name)

    def text(self) -> str:
        return self.body.decode()

    def json(self) -> Any:
        return _json.loads(self.body)


class SyncHttpBackend(Protocol):
    def request(self, request: ApiRequest) -> ApiResponse: ...
    def close(self) -> None: ...


class AsyncHttpBackend(Protocol):
    async def request(self, request: ApiRequest) -> ApiResponse: ...
    async def close(self) -> None: ...


class MissingOptionalDependency(ImportError):
    pass


class BodyNotReplayableError(RuntimeError):
    pass


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
        connection.request(
            str(request.method),
            target,
            body=_body(request.body),
            headers=dict(request.headers),
        )
        response = connection.getresponse()
        observed = ApiResponse(
            response.status, tuple(response.getheaders()), response.read()
        )
        connection.close()
        return observed

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
        send: Callable[[ApiRequest], Awaitable[ApiResponse]],
        close: Callable[[], Awaitable[None]],
    ) -> None:
        self.send = send
        self.closer = close

    async def request(self, request: ApiRequest) -> ApiResponse:
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
            response = session.request(
                str(request.method),
                request.url,
                headers=dict(request.headers),
                data=_body(request.body),
                timeout=request.timeout,
            )
            return ApiResponse(
                response.status_code, tuple(response.headers.items()), response.content
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
            response = pool.request(
                str(request.method),
                request.url,
                headers=dict(request.headers),
                body=_body(request.body),
                timeout=request.timeout,
            )
            return ApiResponse(
                response.status, tuple(response.headers.items()), response.data
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
            response = client.request(
                str(request.method),
                request.url,
                headers=dict(request.headers),
                content=_body(request.body),
                timeout=request.timeout,
            )
            return ApiResponse(
                response.status_code, tuple(response.headers.items()), response.content
            )

        return cls(
            _SyncAdapter(send, client.close if owned else lambda: None), owned=True
        )

    def request(self, request: ApiRequest) -> ApiResponse:
        if self._closed:
            raise RuntimeError("HTTP client is closed")
        return self._backend.request(request)

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

        async def send(request: ApiRequest) -> ApiResponse:
            response = await client.request(
                str(request.method),
                request.url,
                headers=dict(request.headers),
                content=_body(request.body),
                timeout=request.timeout,
            )
            return ApiResponse(
                response.status_code, tuple(response.headers.items()), response.content
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

        async def send(request: ApiRequest) -> ApiResponse:
            timeout = (
                package.ClientTimeout(total=request.timeout)
                if request.timeout is not None
                else package.ClientTimeout()
            )
            async with session.request(
                str(request.method),
                request.url,
                headers=dict(request.headers),
                data=_body(request.body),
                timeout=timeout,
            ) as response:
                return ApiResponse(
                    response.status,
                    tuple(response.headers.items()),
                    await response.read(),
                )

        async def close() -> None:
            if owned:
                await session.close()

        return cls(_AsyncAdapter(send, close), owned=True)

    async def request(self, request: ApiRequest) -> ApiResponse:
        if self._closed:
            raise RuntimeError("HTTP client is closed")
        return await self._backend.request(request)

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


@dataclass(frozen=True, slots=True)
class RetryPolicy:
    max_attempts: int = 1
    initial_delay: float = 0.25
    maximum_delay: float = 8.0
    retry_statuses: tuple[int, ...] = ()


class RetryPolicies:
    DISABLED = RetryPolicy()
    DEFAULT = RetryPolicy(4, retry_statuses=(408, 425, 429, 500, 502, 503, 504))


@dataclass(frozen=True, slots=True)
class RequestOptions:
    token: str | None = None
    headers: Headers = ()
    timeout: float | None = None


@dataclass(frozen=True, slots=True)
class MutationOptions(RequestOptions):
    if_match: str | None = None
    idempotency_key: str | None = None


@dataclass(frozen=True, slots=True)
class CreateFileOptions(MutationOptions):
    media_type: MediaType | str | None = None
    name: GeneratedName | Callable[[ProposedFileName], str] = GeneratedName()


@dataclass(frozen=True, slots=True)
class ProposedFileName:
    folder: str
    default_name: str
    hash: str
    size: int
    token: str


@dataclass(frozen=True, slots=True)
class ByteSplice:
    offset: int
    delete_bytes: int
    insert: ByteSource = b""


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
    token: str
    expires_at: datetime


@dataclass(frozen=True, slots=True)
class MutationReceipt:
    status: int
    location: str
    etag: str
    content_revision: int
    changed: bool
    undo: UndoReceipt | None
    idempotency_key: str | None
    creator_claim: str | None
    management_token: str | None
    response: ApiResponse


@dataclass(frozen=True, slots=True)
class AllocationReceipt:
    mutation: MutationReceipt
    site: str
    path: str
    hash: str
    blob_url: str


@dataclass(frozen=True, slots=True)
class AliasReceipt:
    mutation: MutationReceipt
    path: str
    target: str
    target_kind: str | None
    dangling: bool
    resolved_hash: str | None


@dataclass(frozen=True, slots=True)
class AliasDefinition:
    path: str
    target: str


@dataclass(frozen=True, slots=True)
class FileEntry:
    path: str
    hash: str
    size: int


@dataclass(frozen=True, slots=True)
class AliasInventoryEntry:
    path: str
    target: str
    target_kind: str | None
    dangling: bool
    resolved_hash: str | None
    size: int | None


@dataclass(frozen=True, slots=True)
class FileInventory:
    site: str
    content_revision: int
    tree_hash: str
    etag: str
    files: tuple[FileEntry, ...]
    aliases: tuple[AliasInventoryEntry, ...]


@dataclass(frozen=True, slots=True)
class DirectoryEntry:
    kind: str
    name: str
    files: int | None
    bytes: int
    target: str | None
    target_kind: str | None
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
    kind: str


@dataclass(frozen=True, slots=True)
class ExpiryCap:
    kind: str
    path: str | None
    expires_at: datetime


@dataclass(frozen=True, slots=True)
class ExpiryReport:
    target: ExpiryTarget
    size: int
    refreshed_at: datetime | None
    own_policy: Mapping[str, Any] | None
    inherited_caps: tuple[ExpiryCap, ...]
    effective_expires_at: datetime | None
    remaining_seconds: int | None
    limited_by: Mapping[str, Any] | None


@dataclass(frozen=True, slots=True)
class ExpirySiteReport:
    site: str
    entries: tuple[ExpiryReport, ...]


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
    raw: Mapping[str, Any]


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


class ValidationError(SymbolApiError):
    pass


class UnauthorizedError(SymbolApiError):
    pass


class ForbiddenError(SymbolApiError):
    pass


class NotFoundError(SymbolApiError):
    pass


class MethodNotAllowedError(SymbolApiError):
    pass


class ConflictError(SymbolApiError):
    pass


class PreconditionFailedError(SymbolApiError):
    pass


class PayloadTooLargeError(SymbolApiError):
    pass


class RangeNotSatisfiableError(SymbolApiError):
    pass


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
        case 500:
            error_type = ServerError
        case _:
            error_type = UnexpectedResponseError
    raise error_type(response, key)


def _json_object(response: ApiResponse) -> Mapping[str, Any]:
    value = cast(object, response.json())
    match value:
        case dict():
            return cast(Mapping[str, Any], value)
        case _:
            raise UnexpectedResponseError(response)


def _symbol_stats(value: Mapping[str, Any]) -> SymbolStats:
    return SymbolStats(
        sites=int(value.get("sites", 0)),
        files=int(value.get("files", 0)),
        aliases=int(value.get("aliases", 0)),
        blobs=int(value.get("blobs", 0)),
        bytes=int(value.get("bytes", 0)),
        logical_bytes=int(value.get("logical_bytes", 0)),
        saved_bytes=int(value.get("saved_bytes", 0)),
        saved_fraction=float(value["saved_fraction"]),
        raw=value,
    )


def _date(value: object) -> datetime | None:
    return (
        datetime.fromisoformat(str(value).replace("Z", "+00:00")).astimezone(UTC)
        if value is not None
        else None
    )


def _expiry_report(value: Mapping[str, Any]) -> ExpiryReport:
    target = value["target"]
    return ExpiryReport(
        target=ExpiryTarget(
            site=str(target["site"]),
            path=str(target["path"]) if target.get("path") is not None else None,
            kind=str(target["kind"]),
        ),
        size=int(value["size"]),
        refreshed_at=_date(value.get("refreshed_at")),
        own_policy=value.get("own_policy"),
        inherited_caps=tuple(
            ExpiryCap(
                kind=str(cap["kind"]),
                path=str(cap["path"]) if cap.get("path") is not None else None,
                expires_at=_date(cap["expires_at"]) or datetime.min.replace(tzinfo=UTC),
            )
            for cap in value.get("inherited_caps", ())
        ),
        effective_expires_at=_date(value.get("effective_expires_at")),
        remaining_seconds=(
            int(value["remaining_seconds"])
            if value.get("remaining_seconds") is not None
            else None
        ),
        limited_by=value.get("limited_by"),
    )


def _mutation(response: ApiResponse, key: str | None = None) -> MutationReceipt:
    if response.status < 200 or response.status >= 300:
        _raise(response, key)
    token = response.header("Undo-Token")
    expires = response.header("Undo-Expires")
    undo = (
        UndoReceipt(token, datetime.fromisoformat(expires.replace("Z", "+00:00")))
        if token and expires
        else None
    )
    revision = response.header("Content-Revision")
    return MutationReceipt(
        status=response.status,
        location=response.header("Location") or "",
        etag=response.header("ETag") or "",
        content_revision=int(revision) if revision and revision.isdecimal() else 0,
        changed=undo is not None,
        undo=undo,
        idempotency_key=key,
        creator_claim=response.header("Creator-Claim"),
        management_token=response.header("Management-Token"),
        response=response,
    )


def _allocation(response: ApiResponse, key: str) -> AllocationReceipt:
    mutation = _mutation(response, key)
    value = _json_object(response)
    return AllocationReceipt(
        mutation=mutation,
        site=str(value["site"]),
        path=str(value["path"]),
        hash=str(value["hash"]),
        blob_url=str(value["blob_url"]),
    )


def _alias(response: ApiResponse, key: str) -> AliasReceipt:
    mutation = _mutation(response, key)
    value = _json_object(response)
    return AliasReceipt(
        mutation=mutation,
        path=str(value["path"]),
        target=str(value["target"]),
        target_kind=str(value["target_kind"]) if value.get("target_kind") else None,
        dangling=bool(value["dangling"]),
        resolved_hash=str(value["resolved_hash"])
        if value.get("resolved_hash")
        else None,
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
        retry_policy: RetryPolicy,
    ) -> None:
        self.http = client
        self.origin = origin.rstrip("/")
        self.token = token
        self.retry_policy = retry_policy

    def _send(
        self,
        method: HttpMethod | str,
        url: str,
        *,
        headers: Iterable[tuple[str, str]] = (),
        body: Source | None = None,
        timeout: float | None = None,
    ) -> ApiResponse:
        request = ApiRequest(method, url, tuple(headers), _body(body), timeout)
        attempts = max(1, self.retry_policy.max_attempts)
        for attempt in range(attempts):
            try:
                response = self.http.request(request)
            except OSError:
                if attempt + 1 == attempts:
                    raise
            else:
                version = response.header("Symbol-API-Version")
                revision = response.header("Symbol-API-Revision")
                source_hash = response.header("Symbol-API-Source-Hash")
                if not version or not revision or not source_hash:
                    raise IncompatibleApiVersionError(response)
                if version.split(".", 1)[0] != API_VERSION.split(".", 1)[0]:
                    raise IncompatibleApiVersionError(response)
                if (
                    version == API_VERSION
                    and int(revision) == API_REVISION
                    and source_hash != SOURCE_HASH
                ):
                    raise IncompatibleApiVersionError(response)
                if (
                    response.status not in self.retry_policy.retry_statuses
                    or attempt + 1 == attempts
                ):
                    return response
            cap = min(
                self.retry_policy.maximum_delay,
                self.retry_policy.initial_delay * 2**attempt,
            )
            time.sleep(secrets.randbelow(max(1, int(cap * 1000) + 1)) / 1000)
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
        response = self._send(HttpMethod.GET, self.origin + "/STATS")
        if response.status != 200:
            _raise(response)
        value = _json_object(response)
        return _symbol_stats(value)

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
        if response.status != 200:
            _raise(response)
        value = _json_object(response)
        return FileInventory(
            str(value["site"]),
            int(value["content_revision"]),
            str(value["tree_hash"]),
            response.header("ETag") or "",
            tuple(
                FileEntry(str(item["path"]), str(item["hash"]), int(item["size"]))
                for item in value["files"]
            ),
            tuple(
                AliasInventoryEntry(
                    path=str(item["path"]),
                    target=str(item["target"]),
                    target_kind=str(item["target_kind"])
                    if item.get("target_kind")
                    else None,
                    dangling=bool(item["dangling"]),
                    resolved_hash=str(item["resolved_hash"])
                    if item.get("resolved_hash")
                    else None,
                    size=int(item["size"]) if item.get("size") is not None else None,
                )
                for item in value.get("aliases", ())
            ),
        )

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
                site=str(value["site"]),
                entries=tuple(_expiry_report(item) for item in value["entries"]),
            )
        return _expiry_report(value)

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
        return _expiry_report(_json_object(response))

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
        return _alias(
            self.symbol._send(
                HttpMethod.ALIAS,
                _url(self.symbol.origin, self.name, *path.split("/")),
                headers=headers,
            ),
            key,
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
            [{"path": item.path, "target": item.target} for item in definitions],
            separators=(",", ":"),
        )
        response = self.symbol._send(
            HttpMethod.ALIAS, self.url, headers=headers, body=body
        )
        if response.status not in (200, 201):
            _raise(response, key)
        value = _json_object(response)
        mutation = _mutation(response, key)
        return tuple(
            AliasReceipt(
                mutation=mutation,
                path=str(item["path"]),
                target=str(item["target"]),
                target_kind=str(item["target_kind"])
                if item.get("target_kind")
                else None,
                dangling=bool(item["dangling"]),
                resolved_hash=str(item["resolved_hash"])
                if item.get("resolved_hash")
                else None,
            )
            for item in value["aliases"]
        )

    def copy(
        self, destination: str, options: MutationOptions = MutationOptions()
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Destination", destination))
        return _mutation(
            self.symbol._send(HttpMethod.COPY, self.url, headers=headers), key
        )

    def move(
        self, destination: str, options: MutationOptions = MutationOptions()
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        headers = _options(options)
        headers.append(("Destination", destination))
        return _mutation(self.symbol._send(HttpMethod.MOVE, self.url, headers=headers))

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
        return _allocation(
            self.site.symbol._send(
                HttpMethod.POST,
                _url(
                    self.site.symbol.origin,
                    self.site.name,
                    *self.path.split("/"),
                    trailing=True,
                ),
                headers=headers,
                body=body,
            ),
            key,
        )

    def _custom(
        self,
        body: Source,
        options: CreateFileOptions,
        naming: Callable[[ProposedFileName], str],
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
        value = _json_object(proposal_response)
        proposal = ProposedFileName(
            folder=self.path,
            default_name=str(value["proposed_name"]),
            hash=str(value["hash"]),
            size=int(value["size"]),
            token=str(value["token"]),
        )
        try:
            filename = naming(proposal)
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
        return _allocation(
            self.site.symbol._send(HttpMethod.POST, url, headers=final_headers),
            logical_key,
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
        return _mutation(
            self.site.symbol._send(HttpMethod.PUT, self.url, headers=headers, body=body)
        )

    def remove(self, options: MutationOptions = MutationOptions()) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        return _mutation(
            self.site.symbol._send(
                HttpMethod.DELETE, self.url, headers=_options(options)
            )
        )

    def hash(self) -> str:
        response = self.site.symbol._send(HttpMethod.GET, self.url + "/HASH")
        if response.status != 200:
            _raise(response)
        return response.text().strip()

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
        return _mutation(
            self.site.symbol._send(
                HttpMethod.REPLACE, self.url, headers=headers, body=body
            ),
            key,
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
        return _mutation(
            self.site.symbol._send(
                HttpMethod.PATCH, self.url, headers=headers, body=body
            ),
            key,
        )


class ManagementClient:
    def __init__(self, site: SiteClient) -> None:
        self.site = site

    def action(
        self, action: str, options: MutationOptions = MutationOptions()
    ) -> ApiResponse:
        options = replace(options, token=options.token or self.site.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Management-Action", action))
        response = self.site.symbol._send(
            HttpMethod.MANAGE, self.site.url, headers=headers
        )
        if response.status != 200:
            _raise(response, key)
        return response

    def status(self) -> ManagementStatus:
        return ManagementStatus(bool(_json_object(self.action("status"))["managed"]))

    def claim(self) -> tuple[ManagementStatus, str]:
        response = self.action("claim")
        token = response.header("Management-Token")
        if token is None:
            raise UnexpectedResponseError(response)
        return ManagementStatus(True), token

    def rotate(self) -> tuple[ManagementStatus, str]:
        response = self.action("rotate")
        token = response.header("Management-Token")
        if token is None:
            raise UnexpectedResponseError(response)
        return ManagementStatus(True), token

    def release(self) -> ManagementStatus:
        return ManagementStatus(bool(_json_object(self.action("release"))["managed"]))


class _SymbolAsync:
    def __init__(self, client: AsyncHttpClient, origin: str, token: str | None) -> None:
        self.http, self.origin, self.token = client, origin.rstrip("/"), token

    async def _send(
        self,
        method: HttpMethod | str,
        url: str,
        *,
        headers: Iterable[tuple[str, str]] = (),
        body: Source | None = None,
    ) -> ApiResponse:
        response = await self.http.request(
            ApiRequest(method, url, tuple(headers), _body(body))
        )
        if not response.header("Symbol-API-Version"):
            raise IncompatibleApiVersionError(response)
        return response

    async def request(self, request: ApiRequest) -> ApiResponse:
        return await self.http.request(request)

    async def stats(self) -> SymbolStats:
        response = await self.http.request(
            ApiRequest(HttpMethod.GET, self.origin + "/STATS")
        )
        if response.status != 200:
            _raise(response)
        value = _json_object(response)
        return _symbol_stats(value)

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
        if response.status != 200:
            _raise(response)
        value = _json_object(response)
        return FileInventory(
            str(value["site"]),
            int(value["content_revision"]),
            str(value["tree_hash"]),
            response.header("ETag") or "",
            tuple(
                FileEntry(str(item["path"]), str(item["hash"]), int(item["size"]))
                for item in value["files"]
            ),
            tuple(
                AliasInventoryEntry(
                    path=str(item["path"]),
                    target=str(item["target"]),
                    target_kind=str(item["target_kind"])
                    if item.get("target_kind")
                    else None,
                    dangling=bool(item["dangling"]),
                    resolved_hash=str(item["resolved_hash"])
                    if item.get("resolved_hash")
                    else None,
                    size=int(item["size"]) if item.get("size") is not None else None,
                )
                for item in value.get("aliases", ())
            ),
        )

    async def alias(
        self,
        path: str,
        target: str,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.token)
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Alias-Target", target))
        return _mutation(
            await self.symbol._send(
                HttpMethod.ALIAS,
                _url(self.symbol.origin, self.name, *path.split("/")),
                headers=headers,
            ),
            key,
        )


class AsyncFolderClient:
    def __init__(self, site: AsyncSiteClient, path: str) -> None:
        self.site, self.path = site, path.strip("/")

    async def create(
        self,
        body: Source,
        options: CreateFileOptions = CreateFileOptions(),
    ) -> AllocationReceipt:
        options = replace(options, token=options.token or self.site.token)
        name = options.name
        if callable(name):
            raise TypeError(
                "async custom allocation callbacks use the sync proposal API"
            )
        key = options.idempotency_key or secrets.token_hex(16)
        headers = _options(options, key=key)
        headers.append(("Content-Type", str(options.media_type or MediaTypes.BINARY)))
        if name.prefix:
            headers.append(("File-Prefix", name.prefix))
        if name.suffix:
            headers.append(("File-Suffix", name.suffix))
        if name.extension:
            headers.append(("File-Extension", name.extension.lstrip(".")))
        return _allocation(
            await self.site.symbol._send(
                HttpMethod.POST,
                _url(
                    self.site.symbol.origin,
                    self.site.name,
                    *self.path.split("/"),
                    trailing=True,
                ),
                headers=headers,
                body=body,
            ),
            key,
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
        return response.text()

    async def put(
        self,
        body: Source,
        media_type: MediaType | str = MediaTypes.BINARY,
        options: MutationOptions = MutationOptions(),
    ) -> MutationReceipt:
        options = replace(options, token=options.token or self.site.token)
        headers = _options(options)
        headers.append(("Content-Type", str(media_type)))
        return _mutation(
            await self.site.symbol._send(
                HttpMethod.PUT, self.url, headers=headers, body=body
            )
        )


@overload
def Symbol(
    client: None = None,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolSync: ...


@overload
def Symbol(
    client: HttpClient,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolSync: ...


@overload
def Symbol(
    client: AsyncHttpClient,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolAsync: ...


def Symbol(
    client: HttpClient | AsyncHttpClient | None = None,
    *,
    origin: str = "http://symbol",
    token: str | None = None,
    retry_policy: RetryPolicy = RetryPolicies.DISABLED,
) -> _SymbolSync | _SymbolAsync:
    match client:
        case None:
            return _SymbolSync(HttpClient.stdlib(), origin, token, retry_policy)
        case HttpClient():
            return _SymbolSync(client, origin, token, retry_policy)
        case AsyncHttpClient():
            return _SymbolAsync(client, origin, token)


def _directory(response: ApiResponse) -> DirectoryListing:
    if response.status != 200:
        _raise(response)
    value = _json_object(response)
    return DirectoryListing(
        path=str(value["path"]),
        files=int(value["files"]),
        aliases=int(value.get("aliases", 0)),
        bytes=int(value["bytes"]),
        entries=tuple(
            DirectoryEntry(
                kind=str(item["kind"]),
                name=str(item["name"]),
                files=int(item["files"]) if item.get("files") is not None else None,
                bytes=int(item.get("bytes") or 0),
                target=str(item["target"]) if item.get("target") is not None else None,
                target_kind=str(item["target_kind"])
                if item.get("target_kind")
                else None,
                dangling=bool(item["dangling"])
                if item.get("dangling") is not None
                else None,
            )
            for item in value["entries"]
        ),
    )


__all__ = (
    "API_REVISION",
    "API_VERSION",
    "BUILD_COMMIT",
    "BUILD_DIRTY",
    "GENERATOR_VERSION",
    "METADATA",
    "SOURCE_HASH",
    "AbsoluteExpiry",
    "AliasDefinition",
    "AliasInventoryEntry",
    "AliasReceipt",
    "AllocationReceipt",
    "ApiMetadata",
    "ApiRequest",
    "ApiResponse",
    "AsyncHttpClient",
    "ByteSplice",
    "Charset",
    "ContentFormat",
    "ContentFormats",
    "CreateFileOptions",
    "DecayExpiry",
    "DirectoryEntry",
    "DirectoryListing",
    "ExpiryCap",
    "ExpiryReport",
    "ExpirySiteReport",
    "ExpiryTarget",
    "FileEntry",
    "FileInventory",
    "GeneratedName",
    "HttpClient",
    "HttpMethod",
    "ManagementStatus",
    "MediaType",
    "MediaTypes",
    "MissingOptionalDependency",
    "MutationOptions",
    "MutationReceipt",
    "NeverExpiry",
    "RelativeExpiry",
    "RequestOptions",
    "RetryPolicies",
    "RetryPolicy",
    "Symbol",
    "SymbolApiError",
    "SymbolStats",
)

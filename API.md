<!-- API:INDEX:START -->
# Symbol API

Symbol serves static sites and embeds clients for browsers, TypeScript,
JavaScript, Python 3.14, POSIX shell, and raw HTTP.

- [JavaScript and TypeScript](/API/JS)
- [Python 3.14](/API/PY)
- [Shell client](/API/SH)
- [HTTP, curl, and protocol reference](/API/CURL)

Downloadable clients:

- [`/symbol.ts`](/symbol.ts), [`/symbol.js`](/symbol.js),
  [`/symbol.global.js`](/symbol.global.js), and [`/symbol.d.ts`](/symbol.d.ts)
- [`/symbol.py`](/symbol.py)
- [`/symbol.sh`](/symbol.sh)

Every client preserves the server's existing authorization rule: unmanaged
sites are freely mutable; managed sites require a management token. Client
retries do not weaken optimistic concurrency or idempotency.
<!-- API:INDEX:END -->

<!-- API:JS:START -->
# JavaScript and TypeScript

## Installation and module formats

Use the ES module directly:

```js
import {
  MediaTypes,
  RetryPolicies,
  SymbolClient,
} from "https://symbol.example/symbol.js";

await using symbol = new SymbolClient({
  origin: "https://symbol.example",
  retryPolicy: RetryPolicies.Default,
});

const site = symbol.site("notes");
const text = await site.file("index.html").text();
```

Browsers without modules may load `/symbol.global.js`; its identical exports are
available synchronously from `globalThis.SymbolAPI`. TypeScript may import
`/symbol.ts` directly or pair `/symbol.js` with `/symbol.d.ts`.

The hierarchy is `SymbolClient → SiteClient → FolderClient/FileClient`.
Root methods cover stats, listing, unnamed creation, and the raw request escape
hatch. Site methods cover inventory, aliases, archives, copy/move, undo,
expiry, management, and conditional publication. File methods cover reads,
typed decoding, PUT, DELETE, whole replacement, and byte splicing.

Complete method inventory:

- `SymbolClient`: `request`, `docs`, `docsHash`, `installer`,
  `installerHash`, `shellClient`, `shellClientHash`, `apiClient`,
  `apiClientHash`, `apiManual`, `apiVersion`, `stats`, `sites`, `create`,
  `site`, `blob`, `assertExactApi`, and async disposal.
- `SiteClient`: `file`, `folder`, `redirect`, `get`, `put`, `remove`,
  `archive`, `pop`, `files`, `alias`, `aliases`, `copy`, `move`, `undo`,
  `undoStack`, `setExpiry`, `expiry`, and `management`.
- `FolderClient`: `bytes`, `text`, `json`, `html`, `blob`, and `create`.
- `FileClient`: `get`, `bytes`, `text`, `json`, `put`, `putJson`, `replace`,
  `splice`, `patch`, `remove`, `hash`, `setExpiry`, and `expiry`.
- `ManagementClient`: `status`, `claim`, `rotate`, and `release`.
- `Operation`: promise-compatible `then`, `retry`, `abort`, status/history
  accessors, and async disposal.

Typed value/resource APIs additionally expose `apiIdentity`, `origin`,
`attempts`, `canRetry`, `replayable`, `parse`, `application`, `from`, `text`,
`image`, `audio`, `video`, `font`, `utf8`, `essence`, `structuredSuffix`,
`charset`, `parameter`, `withParameter`, `withCharset`, `withoutParameter`,
`equals`, and `toString`.

`apiClient` accepts only `symbol.ts`, `symbol.js`, `symbol.global.js`,
`symbol.d.ts`, and `symbol.py`. `apiManual` accepts only `index`, `javascript`, `typescript`,
`python`, `shell`, and `protocol`.
Metadata parses semantic versions into readonly `[major, minor, patch]`
`ApiVersion` tuples and validates branded `Blake3` and `GitCommit` values;
`API_VERSION` remains the canonical wire-format string.

## SymbolClient reference

### `new SymbolClient(options)`

`origin` defaults to the current origin. `token` supplies a management token,
`creatorClaim` supplies a recovery identity, `fetch` replaces the transport,
and `retryPolicy` selects automatic retry behavior. Values remain in memory;
the client never writes credentials.

### `request(path, init)`

Performs a raw request while preserving API identity checks, cancellation,
attempt history, response ownership, and typed compatibility failures. Use it
only when an operation has no higher-level method.

```ts
await using response = await symbol.request("/demo/custom", {
  method: "GET",
  headers: { Accept: "application/octet-stream" },
});
```

### `stats()` and `sites()`

`stats()` returns exact storage distributions and serving metrics. `sites()`
returns a cache-aware discriminated directory listing; status `304` has no
invented listing body.

```ts
const totals = await symbol.stats();
console.log(totals.sites, totals.savedBytes, totals.serving.cache.hits);

const listing = await symbol.sites();
if (listing.status === 200) {
  for (const entry of listing.entries) console.log(entry.kind, entry.name);
}
```

### `create(body, options)`

Creates a random site. The SDK generates one idempotency key per logical
operation unless supplied. `managed: true` requests write protection at
creation. `mediaType`, `filename`, and `unpack` describe the body.

### `site(name)` and `blob(site, hash)`

`site(name)` returns a hierarchy without making a request. `blob` reads a
site-scoped immutable content-addressed resource and supports ranges and
streaming.

### Built-in resources

`docs`, `installer`, `shellClient`, `apiClient`, and `apiManual` return
cache-aware text assets. Their matching `*Hash` methods return raw Blake3
digests. `apiVersion` returns the server version tuple, absolute revision,
source hash, git commit, and dirty bit.

### `apiIdentity` and `assertExactApi()`

`apiIdentity` is null before the first response. Compatible monotonic
minor/patch upgrades are accepted during a client session; major mismatch,
rollback, and impossible equal-version/equal-revision hash changes fail.
`assertExactApi()` additionally requires this generated client’s exact build.

## SiteClient reference

### `get()` and `redirect()`

`get()` reads the site index as an owned streaming response. `redirect()`
observes the canonical slash redirect without automatically following it.

### `put(body, options)`

Merges an archive, directory encoding, or file into the site. It does not
delete omitted paths. `ifMatch` provides optimistic whole-site concurrency.

### `files(path?)`

With no path, returns the exact inventory: files, aliases, content revision,
tree hash, creation/update timestamps, the publish timeline, and cache
metadata. With a path, returns a browsable subtree.

### `archive(format?)` and `pop(format?)`

`archive` downloads `tar.gz`, `tar`, or `zip` without mutation. `pop`
atomically removes the site and returns the archive plus undo metadata.
Dispose unread bodies explicitly.

```ts
await using archive = await symbol.site("demo").archive("zip");
const bytes = await archive.arrayBuffer();
```

### `copy(destination?, options)` and `move(destination)`

Copy refuses an occupied destination and may request a generated destination.
Move renames without transferring blob bytes. Generated copy operations retain
one idempotency key.

### `alias(path, target)` and `aliases(definitions)`

The single method creates one live path alias. The batch method validates and
commits every definition atomically. Targets are site-relative paths, not
external URLs.

### `undo(token?)` and `undoStack()`

`undoStack` returns retained tokens with typed kind, description, creation,
expiry, and remaining duration. `undo` defaults to the newest applicable
entry, or accepts an explicit token.

### `setExpiry(expiry, path?)` and `expiry(path?)`

Expiry can be relative, absolute, decay-based, or never. Reports distinguish
owned policy, inherited caps, effective deadline, and the limiting target.

### `management()`

Returns `ManagementClient`. `status` is read-like; `claim`, `rotate`, and
`release` change write ownership. Reads always remain public.

## FolderClient reference

### Typed allocation helpers

`bytes`, `text`, `json`, `html`, and `blob` infer media type and extension.
`create` accepts an explicit body and `CreateFileOptions`.

Name constraints contain optional `prefix`, `suffix`, and `extension`.
Identical content and constraints naturally deduplicate to one hash-derived
path.

### Custom naming callback

When `name` is a callback, Symbol proposes a hash, size, media type, inferred
extension, and default name. The callback returns a basename. Callback failure
cancels the proposal and makes that failed operation non-retryable.

```ts
const receipt = await site.folder("reports").create(pdf, {
  mediaType: MediaTypes.Pdf,
  name: async (proposal) => `report-${proposal.hash.slice(-12)}.pdf`,
});
```

## FileClient reference

### `get(options)`, `bytes()`, `text()`, and `json<T>()`

`get` exposes the owned response and supports `range`, `ifRange`, and
`ifNoneMatch`. Convenience decoders require a successful response and validate
the expected representation.

### `put(body, options)` and `putJson(value, options)`

PUT adds or replaces this ordinary path. It intentionally does not advertise
idempotent replay. `putJson` serializes once and uses JSON media type.

### `replace(body, options)`

REPLACE requires `baseHash`, atomically replaces the whole content, and may
relocate a content-addressed allocated filename. The typed receipt
distinguishes replaced, relocated, and unchanged outcomes.

### `splice(change, options)` and `patch(changes, options)`

Splices use offsets into the original file, so arbitrary insert/delete/replace
diffs are possible. Small operations use headers; larger operations use the
framed binary format automatically. All ranges validate before commit.

### `remove()`, `hash()`, and expiry

`remove` deletes this path and returns undo metadata. `hash` returns the raw
content Blake3. `setExpiry` and `expiry` operate on this exact target.

## Allocated content-addressed files

```ts
const receipt = await symbol
  .site("nlab")
  .folder("annotations/pages/some-page")
  .bytes(annotationBytes, {
    name: { prefix: "comment-", extension: "anno" },
  });

console.log(receipt.path);       // hash-derived mutable site path
console.log(receipt.blobUrl);    // immutable content-addressed URL
console.log(receipt.hash);       // lowercase Blake3
```

Identical content and naming constraints resolve to the same path. For a
custom proposed name, pass the synchronous proposal callback accepted by
`create`; the server rejects a callback result that is invalid or already
taken. Async clients use prefix, suffix, and extension constraints.

## Aliases and atomic changes

```ts
const site = symbol.site("notes");
await site.alias("latest", "releases/current");
await site.aliases([
  { path: "docs", target: "releases/current/docs" },
  { path: "download", target: "releases/current/app.zip" },
]);

await site.file("data.bin").replace(newBytes, { baseHash });
await site.file("data.bin").patch(
  [
    { offset: 8, deleteBytes: 4, insert: replacement },
    { offset: 64, deleteBytes: 0, insert: trailer },
  ],
  { baseHash },
);
```

Aliases have symlink semantics and follow the target path. Reads use the
cached resolved hash; writes follow the target; dangling aliases remain
visible; cycles and namespace shadowing are rejected. Batch alias creation and
multi-splice are atomic.

Operations are awaitable and retryable. The default retry policy is disabled;
`RetryPolicies.Default` uses exponential backoff with full jitter for safe,
replayable failures. Each logical idempotent operation generates one key when
none is supplied and retains it across retries. `SymbolApiError` preserves the
status, headers, bytes, response, and idempotency key. Owned clients,
responses, streams, proposals, and operations support `using` or
`await using`; borrowed transports are never closed by the SDK.

## Receipts and errors

Mutation receipts expose status, location, entity tag, content revision,
sanitization counts, replay state, raw response, and either an `UndoReceipt` or
null. Specialized receipts add exact allocation, alias, replacement, splice,
archive, management, and creation fields.

`SymbolApiError` preserves operation name, status, body, idempotency key, and a
cloned response. Typed subclasses cover validation, authorization, forbidden
identity, missing resources, unsupported methods, conflicts, stale
preconditions, payload limits, ranges, server failures, malformed responses,
unexpected responses, unavailable operations, incompatible versions, and API
integrity failures.

```ts
try {
  await site.file("data.bin").replace(next, { baseHash: staleHash });
} catch (error) {
  if (error instanceof PreconditionFailedError) {
    console.error(error.etag, error.contentRevision);
  }
}
```

An `Operation` records immutable attempts with start/end time, status, and
error. Automatic retry requires a safe endpoint and replayable body. Manual
retry is available only after failure. Successful, disposed, aborted, callback
failed, and non-replayable operations reject retry.

## Complete workflow

<!-- EXEC:TS:START -->
```ts
import {
  MediaTypes,
  RetryPolicies,
  SymbolClient,
} from "./symbol.js";

const origin = new URL(import.meta.url).searchParams.get("origin");
if (!origin) throw new Error("the module URL needs an origin query parameter");
await using symbol = new SymbolClient({
  origin,
  retryPolicy: RetryPolicies.Default,
});

const created = await symbol.create("<h1>demo</h1>", {
  mediaType: MediaTypes.Html,
});
const site = symbol.site(new URL(created.location).pathname.split("/")[1]);

await site.file("config.json").putJson({ enabled: true });
await site.alias("latest", "config.json");

const inventory = await site.files();
if (inventory.status === 200) {
  console.log(inventory.treeHash, inventory.aliases);
}

const baseHash = await site.file("config.json").hash();
await site.file("config.json").replace('{"enabled":false}', {
  baseHash,
  mediaType: MediaTypes.Json,
});

const stack = await site.undoStack();
if (stack.entries.length > 0) await site.undo(stack.entries[0].token);
```
<!-- EXEC:TS:END -->
<!-- API:JS:END -->

<!-- API:PYTHON:START -->
# Python 3.14

## Installation and imports

Download the dependency-free generated module and import it directly:

```sh
curl -fsS "${SYMBOL_BASE}/symbol.py" -o symbol_api.py
```

The stdlib client is synchronous:

```python
from symbol_api import MediaTypes, Symbol

with Symbol(origin="https://symbol.example") as symbol:
    site = symbol.site("notes")
    site.file("index.html").put(
        "<h1>Notes</h1>",
        media_type=MediaTypes.HTML,
    )
    print(site.file("index.html").text())
```

## Synchronous client reference

### `Symbol(...)`

Without a transport, `Symbol` owns a stdlib client. Pass `HttpClient` to select
another backend. `origin`, `token`, `creator_claim`, and `retry_policy` apply
to the hierarchy. Leaving a `with` block closes only owned transports.

### Root operations

`stats` returns exact `SymbolStats`, including `SizeDistribution`,
`ServingStats`, `CacheStats`, and `ReaderStats`. `sites` returns
`DirectoryListing`. `create` publishes an unnamed site. `site` constructs a
site client. `api_client`, `api_client_hash`, `api_manual`, and `api_version`
read built-in resources. `api_version` returns `ApiVersionDocument`, including
the server git commit and dirty bit. `request` is the raw escape hatch.

```python
from symbol_api import ApiClientAsset, ApiManual, Symbol

with Symbol(origin=origin) as symbol:
    print(symbol.stats().serving.readers.query_micros)
    print(symbol.api_client_hash(ApiClientAsset.PYTHON))
    print(symbol.api_manual(ApiManual.PROTOCOL).text())
```

### Site operations

`files` returns the exact file/alias inventory. `archive` downloads without
mutation; `pop` downloads and removes. `copy` and `move` refuse occupied
destinations. `alias` creates one live path alias and `aliases` commits a typed
batch atomically. `undo` applies a retained operation; `undo_stack` lists
available entries. `set_expiry` and `expiry` manage retention.
`management()` returns claim/status/rotate/release operations.

### Folder operations

`bytes`, `text`, and `json` allocate hash-derived files with inferred formats.
`create` accepts `CreateFileOptions`, including generated naming constraints or
a custom proposal callback.

```python
from symbol_api import CreateFileOptions, GeneratedName, MediaTypes

receipt = (
    symbol.site("notes")
    .folder("events")
    .create(
        b"opaque event",
        CreateFileOptions(
            media_type=MediaTypes.BINARY,
            name=GeneratedName(prefix="event-", extension="bin"),
        ),
    )
)
print(receipt.path, receipt.hash, receipt.blob_url)
```

### File operations

`get` returns a closeable streaming `ApiResponse`. `bytes`, `text`, and `json`
consume and decode it. `put` writes ordinary content. `remove` returns
`DeleteReceipt`. `hash` returns validated `Blake3`. `replace` requires the
current content hash. `splice` applies one edit and `patch` applies multiple
original-offset edits atomically.

```python
from symbol_api import ByteSplice

file = symbol.site("notes").file("data.bin")
base = file.hash()
file.patch(
    (
        ByteSplice(offset=4, delete_bytes=2, insert=b"XY"),
        ByteSplice(offset=10, delete_bytes=0, insert=b"tail"),
    ),
    base_hash=base,
)
```

`HttpClient.stdlib()`, `.requests()`, `.urllib3()`, and `.httpx()` normalize
the supported synchronous transports. `AsyncHttpClient.httpx()` and
`.aiohttp()` select asynchronous operation; the overloaded `Symbol(...)`
factory returns the matching sync or async hierarchy.

## Asynchronous client reference

Passing `AsyncHttpClient` selects `_SymbolAsync`; every root, site, folder,
file, and management operation has the same public method name and typed
result as its synchronous counterpart. Network access, response streaming,
retry delays, and custom naming callbacks remain asynchronous.

```python
import asyncio
from symbol_api import AsyncHttpClient, CreateFileOptions, GeneratedName, Symbol

async def main() -> None:
    async with Symbol(
        AsyncHttpClient.aiohttp(),
        origin="https://symbol.example",
    ) as symbol:
        receipt = await (
            symbol.site("nlab")
            .folder("annotations/pages/some-page")
            .create(
                b"annotation bytes",
                CreateFileOptions(
                    name=GeneratedName(prefix="comment-", extension="anno"),
                ),
            )
        )
        print(receipt.path, receipt.blob_url)

asyncio.run(main())
```

Async request bodies accept bytes, text, or `AsyncIterable[bytes]`. They reject
`Path` and synchronous readers instead of hiding blocking work in a thread.
Async splice operations use `AsyncByteSplice`; insertion values follow the same
non-blocking rule and are materialized before the atomic request is sent.

```python
async def chunks():
    for part in (b"first", b"second", b"third"):
        yield part

async with Symbol(AsyncHttpClient.httpx(), origin=origin) as symbol:
    receipt = await symbol.site("notes").file("stream.bin").put(chunks())
    response = await symbol.site("notes").file("stream.bin").get()
    async for chunk in response.aiter_bytes(64 * 1024):
        consume(chunk)
```

Use `await response.aread()`, `.atext()`, `.ajson()`, or iterate incrementally.
`aabort`, `aclose`, and async context-manager exit release owned response
resources exactly once.

## Transport backends

`HttpClient.stdlib()` has no dependency. `requests`, `urllib3`, and synchronous
`httpx` constructors import those packages lazily. Async clients support
`httpx` and `aiohttp`. `wrap` accepts a caller-owned implementation of the
typed backend protocol.

Owned constructors close their pool/session/client. Wrapping an existing
backend or passing an existing third-party session leaves it open.

Transport normalization catches only documented network and timeout errors.
Programming, validation, and custom backend exceptions pass through unchanged.

Uploads stream file readers synchronously or native asynchronous iterables;
downloads expose streaming response resources. Structured protocol responses
are buffered only when decoding requires their complete body.

## Models and errors

Metadata uses `ApiArtifact`, parsed `ApiVersion`, validated `Blake3`,
`TreeHash`, `EntityTag`, `GitCommit`, `ManagementToken`, `CreatorClaim`, and
`UndoToken` values rather than anonymous structured strings.

Inventory, directory, statistics, undo, expiry, mutation, allocation, alias,
delete, management, request-attempt, and API identity responses are frozen,
slotted dataclasses. Discriminants use enums such as `DirectoryKind`,
`AliasTargetKind`, `ExpiryKind`, `UndoKind`, `ManagementAction`,
`ApiClientAsset`, and `ApiManual`.

`MalformedResponseError` rejects missing, extra, mistyped, non-finite, or
inconsistent structured fields. `UnauthorizedError.challenge`,
`PreconditionFailedError.etag/content_revision`, and
`RangeNotSatisfiableError.content_range` expose required protocol details.

### Response resource lifecycle

`ApiResponse` is a context manager. `read`/`aread` buffer the remainder once;
`iter_bytes`/`aiter_bytes` stream incrementally; `close`/`aclose` release the
connection without consuming it. `text`, `json`, `atext`, and `ajson` are
convenience decoders.

```python
with symbol.site("media").file("movie.mp4").get() as response:
    for chunk in response.iter_bytes(256 * 1024):
        write_chunk(chunk)
```

Every failed replayable response retains `attempts`, `replayable`,
`idempotency_key`, and `retry`/`aretry`. Typed statistics, allocation, alias,
and content-mutation failures also expose their stateful `operation`; retry
returns the original typed result rather than a raw response. A successful
manual retry moves that operation to `OperationPhase.SUCCEEDED` and cannot be
reused. Terminal network failures preserve the same logical request.
Successful, aborted, disposed, and non-replayable operations reject manual
retry. `Retry-After` never exceeds `RetryPolicy.maximum_delay`.

Optional backends import lazily; importing `/symbol.py` requires only Python
3.14. Models are frozen, slotted dataclasses. Request methods use exact receipt
and error classes; raw arbitrary JSON alone remains dynamically typed.
Management tokens remain caller-owned in-memory values. Context-manager exit
closes owned transports exactly once and leaves borrowed clients open.

Synchronous uploads and `ByteSplice` insertions accept bytes, text, `Path`, and
`ByteReader`. Async uploads and `AsyncByteSplice` insertions accept bytes, text,
or a native `AsyncIterable[bytes]`; they deliberately reject `Path` and
synchronous readers instead of hiding blocking file I/O in worker threads.
Responses stream through `iter_bytes`/`aiter_bytes`.

Successful operations cannot be retried. Failed replayable HTTP and network
operations preserve immutable attempt history, idempotency identity, and
`retry`/`aretry`; every automatic or manual delay, including `Retry-After`, is
capped by the configured maximum.

Both sync and async roots provide `stats`, `sites`, `site`, `api_client`,
`api_client_hash`, `api_manual`, `api_version`, and raw `request`. Site
clients provide `file`, `folder`, `files`, `alias`, `aliases`, `copy`, `move`,
`archive`, `pop`, `undo`, expiry, and management operations. Folder clients
provide `create`, `bytes`, `text`, and `json`; file clients provide `get`,
`bytes`, `text`, `json`, `put`, `remove`, `hash`, `replace`, and `splice`.
Use `ApiClientAsset` and `ApiManual` enums rather than unvalidated strings for
the built-in resources.
`ApiVersion` is a parsed immutable value object, while `Blake3` and
`GitCommit` validate in their constructors; wire-format string constants are
derived from those values.

Transport and typed-value APIs additionally expose `stdlib`, `requests`,
`urllib3`, `httpx`, `aiohttp`, `wrap`, `close`, `header`, `parse`,
`application`, `image`, `essence`, `charset`, `parameter`, `with_parameter`,
and response-body `body`, `read`, `iter_bytes`, `aread`, `atext`, `ajson`,
`aiter_bytes`, `retry`, `aretry`, `abort`, `aabort`, and `aclose` operations.
Responses and typed HTTP errors retain an immutable `attempts` tuple and a
`replayable` flag; manual retry preserves the prior attempt history.
Site resources also expose `url`, `undo_stack`, `set_expiry`, `expiry`, and
`management`; management clients expose `action`, `status`, `claim`, `rotate`,
and `release`; file clients expose `patch`.

## Complete workflow

<!-- EXEC:PYTHON:START -->
```python
import os

from symbol_api import (
    AliasDefinition,
    CreateFileOptions,
    GeneratedName,
    MediaTypes,
    PublishOptions,
    RetryPolicies,
    Symbol,
)

origin = os.environ["SYMBOL_BASE"]

with Symbol(origin=origin, retry_policy=RetryPolicies.DEFAULT) as symbol:
    created = symbol.create(
        "<h1>Python demo</h1>",
        PublishOptions(media_type=MediaTypes.HTML),
    )
    site_name = created.location.rstrip("/").rsplit("/", 1)[-1]
    site = symbol.site(site_name)

    site.file("config.json").put(
        '{"enabled":true}',
        media_type=MediaTypes.JSON,
    )
    site.aliases(
        (
            AliasDefinition("latest", "config.json"),
            AliasDefinition("current", "latest"),
        )
    )

    allocated = site.folder("events").json(
        {"kind": "created"},
        CreateFileOptions(
            name=GeneratedName(prefix="event-", extension="json"),
        ),
    )
    print(allocated.path)

    inventory = site.files()
    print(inventory.tree_hash, inventory.aliases)

    current_hash = site.file("config.json").hash()
    site.file("config.json").replace(
        '{"enabled":false}',
        base_hash=current_hash,
        media_type=MediaTypes.JSON,
    )

    stack = site.undo_stack()
    if stack.entries:
        site.undo(stack.entries[0].token)
```
<!-- EXEC:PYTHON:END -->
<!-- API:PYTHON:END -->

<!-- API:SHELL:START -->
# POSIX shell client

## Installation and configuration

Download:

```sh
curl -fsS "${SYMBOL_BASE}/symbol.sh" -o symbol
chmod +x symbol
```

`SYMBOL_HOST` selects the server and defaults to `http://symbol`.
`SYMBOL_TOKEN` supplies a management token. `-t/--token` takes precedence and
may appear before or after the command, before `--`.
`SYMBOL_STDIN` is `tty` (default), `always`, or `never`: a pipe without `-`
is stdin only when no file source is given, and only in a terminal unless
set to `always`. `never` requires `-`.
`SYMBOL_NO_UPDATE_CHECK=1` skips the client update nag. The nag is also
throttled to once per 24 hours.

`-` means stdin in an upload/source position and stdout in a download
destination position. The client keeps binary stdout clean so archives can be
piped safely.

Every proper command supports focused help:

```sh
symbol remix --help
symbol help remix
```

## Command reference

### `put`

```sh
symbol put [-u|--unpack] [--replace] [--managed] [--json] [NAME [FILE [DEST]]]
command | symbol put [NAME] -
```

A pipe without `-` is stdin only when no file source is given and stdout
or stderr is a terminal. `SYMBOL_STDIN=always` treats any non-terminal
stdin as a pipe; `SYMBOL_STDIN=never` requires `-`. Scripts and CI should
pass `-` or set `never`.

Without `NAME`, Symbol creates a random site. A single HTML input becomes
`index.html`. Directories are packed and unpacked automatically. `-u` unpacks
tar, tar.gz, zip, or gzip. Publishing to an existing site merges only the
supplied paths. `--replace` replaces the whole site tree and deletes remote
paths that are not in the upload; generated `symbol.toml` is kept. There is
no prompt. `--json` prints a mutation envelope.

### `clone`

```sh
symbol clone NAME [DIR]
```

Downloads and extracts a site, validates its manifest, then creates aliases as
safe relative symlinks. If symlinks are unavailable, it materializes safe
copies while preserving enough metadata to re-upload them as aliases.

### `get` and `pop`

```sh
symbol get NAME [ARCHIVE|-]
symbol pop NAME [ARCHIVE|-]
```

Both support tar.gz, tar, and zip. `get` leaves the site untouched. `pop`
removes it only as part of the same successful server operation and prints its
undo command.

### `copy`, `move`, and `remix`

```sh
symbol copy [--managed] SRC [DST]
symbol move SRC DST
symbol remix [--managed] SRC [DST]
```

Copy duplicates server metadata and reuses blob content. Move renames without
copying files. Remix performs a copy and then clones the result locally; if the
clone fails, it reports the retained server copy and exact cleanup command.

### `alias`

```sh
symbol alias [--json] SITE PATH TARGET [PATH TARGET ...]
```

One pair creates one live alias. Multiple pairs are sent as one atomic batch.
Targets are site-relative and follow symlink semantics. Cycles, root escapes,
reserved namespaces, and path shadowing are rejected. `--json` writes the
server receipt.

### `sync`

```sh
symbol sync
symbol sync --check
```

Sync requires a local `symbol.toml` checkout. It compares local changes with
the checkout baseline and current upstream tree. It refuses divergent
upstream changes instead of overwriting them and does not delete remote
paths that are missing locally. `--check` prints the proposed diff without
publishing.

### `undo`

```sh
symbol undo [--json] [NAME [TOKEN]]
symbol undo --stack [--json] [NAME]
```

Without a token, reverses the newest applicable retained mutation. `--stack`
shows token, action, creation time, expiry time, and remaining duration.
`--json` writes the stack document or a mutation envelope.

### `expire`

```sh
symbol expire
symbol expire [--json] NAME [PATH] --in DURATION
symbol expire [--json] NAME [PATH] --at RFC3339
symbol expire [--json] NAME [PATH] --decay [LIMITS]
symbol expire [--json] NAME [PATH] --never
symbol expire [--json] NAME [PATH] --show
```

Bare `expire` prints the retention graph and help. Relative and absolute modes
set direct deadlines. Decay combines minimum age, maximum age, maximum size,
and power. Child targets inherit limiting parent caps. `--json` writes the expiry report.

### `manage`

```sh
symbol manage [--json] NAME --status
symbol manage [--json] NAME --claim
symbol manage [--json] NAME --rotate
symbol manage [--json] NAME --release
```

Management is optional write protection. Reads remain public. Claim and rotate
return a token once; release makes writes open again. `--json` writes the
`{"managed":…}` body only.

### `ls`, `stats`, `url`, and `api`

```sh
symbol ls [--json]
symbol ls -l [--json] NAME
symbol stats [--json]
symbol url NAME
symbol api
symbol api --json
```

`ls` lists sites or one site tree from listing/inventory JSON fields, so
names are never parsed out of the table. `-l` adds URLs. `ls -l --json` is
the same as `ls --json`. `stats` shows logical/physical bytes, deduplication,
distributions, cache, and reader metrics. `url` prints one canonical site
URL for scripting. `api` prints the live `/API/VERSION` document: semantic
version, absolute revision, source hash, git commit, and dirty bit. `--json`
writes the server JSON.

### `rm`

```sh
symbol rm [--json] NAME [PATH]
```

Deletes without downloading an archive and without a prompt. A path removes
only that file or subtree; omitting it removes the site. The server still
returns retained undo metadata for 4 hours. `--json` prints a mutation
envelope.

### `recover`, `update`, and `help`

```sh
symbol recover
symbol update
symbol help [COMMAND]
```

Recover replays interrupted generated PUT/COPY operations with their original
idempotency identity. Update reinstalls the client and reports whether its
hash changed. Help prints general or focused command usage.

Core workflows:

```sh
symbol put index.html
symbol put notes ./dist
symbol sync
symbol clone notes
symbol get notes
symbol copy notes notes-copy
symbol move notes-copy notes-archive
symbol alias notes latest releases/current
symbol rm notes old.txt
symbol undo --stack notes
symbol expire notes --in 30d
symbol manage notes --claim
```

`put` merges and never implicitly wipes unrelated paths. `sync` compares the
checkout manifest and refuses upstream drift. `get` downloads without
deleting; `pop` downloads and deletes; `rm` deletes without backup. A file
argument of `-` reads stdin for upload and writes stdout for download.
Archives preserve aliases as symlinks when supported and materialize safe
copies otherwise.

Commands resolve by unique proper-name prefix, then by substring only when no
prefix matches. An ambiguous input reports only distinct proper operations.
Proper command names are `put`, `clone`, `get`, `pop`, `copy`, `remix`,
`move`, `alias`, `api`, `sync`, `undo`, `expire`, `manage`, `recover`, `ls`,
`rm`, `url`, `stats`, `update`, and `help`.
Aliases are footnotes, not additional proper commands:

1. `push` → `put`
2. `pull` → `clone`
3. `rename` → `move`
4. `add` → `put`
5. `x` → `remix`
6. `list` → `ls`
7. `download` → `get`
8. `delete` → `rm`
9. `upgrade` → `update`

Managed sites read tokens from `--token`, then `SYMBOL_TOKEN`, then the
mode-0600 checkout manifest. Generated PUT/COPY operations retain pending
idempotency records; `symbol recover` safely replays them after a dropped
response.

## Checkout and sync workflow

```sh
symbol clone notes
cd notes

# edit files
printf '%s\n' '<h1>updated</h1>' > index.html

symbol sync --check
symbol sync
```

Clone writes the baseline used by sync and excludes local secret sidecars from
uploads. Sync uploads additions and changes while preserving remote paths not
present in the partial local archive. If upstream changed, clone it separately,
resolve locally, then publish again.

Bare `symbol put` inside a checkout publishes the nearest project. It is less
strict than sync: put merges what you provide, while sync verifies the recorded
upstream baseline first.

## Management and recovery

```sh
symbol put --managed private-demo ./dist
symbol manage private-demo --status
symbol manage private-demo --rotate
```

The client selects a token from explicit `--token`, `SYMBOL_TOKEN`, then the
checkout’s protected sidecar. Recognizable Symbol credentials are redacted
from inspectable uploads.

Generated destinations use persisted idempotency state. If the server commits
but the connection drops before the client receives the generated name or
one-time credential, run:

```sh
symbol recover
```

Recovery replays the same logical operation; it does not create another site.
Records older than seven days are pruned.

## Complete workflow

<!-- EXEC:SHELL:START -->
```sh
# Publish a folder to a chosen name.
symbol put demo ./dist

# Add one file without replacing the rest.
symbol put demo ./robots.txt

# Inspect and clone.
symbol ls -l demo
symbol clone demo demo-work

# Safely publish local edits.
cd demo-work
symbol sync --check
symbol sync

# Create a server-side copy and work on it.
cd ..
symbol remix demo demo-next

# Add a live alias and optional expiry.
symbol alias demo-next latest index.html
symbol expire demo-next assets/demo.mp4 --in 30d

# Inspect or reverse retained changes.
symbol undo --stack demo-next
symbol undo demo-next

# Download without deleting.
symbol get demo-next demo-next.zip
```
<!-- EXEC:SHELL:END -->
<!-- API:SHELL:END -->

<!-- API:PROTOCOL:START -->
# HTTP and curl protocol

This is the implementation reference for the current public HTTP surface.
Examples use `SYMBOL_BASE=https://symbol.example`; set it to the configured
`SYMBOL_PUBLIC_URL`.

`symbol contract` emits the typed route, method, header, and status inventory
used by conformance tests for this document.

<!-- contract:api documentation -->
### `GET /API`

Returns `307 Temporary Redirect` with `Location: /API/`. Mutation methods are
rejected with `405` and `Allow: GET, HEAD`.

### `GET /API/{manual}`

`/API/`, `/API/JS`, `/API/PY`, `/API/SH`, and `/API/CURL` are compiled from
the marked sections in this file. JS/TS, PY/PYTHON, SH, and
CURL/HTTP/REST/PROTOCOL aliases return identical bytes, ETags, cache policy,
and canonical `Link`. HTML is negotiated for browsers; explicit Markdown or
plain requests receive the canonical section. Responses use `no-cache`,
`Vary: Accept, User-Agent`, and support `If-None-Match`.

The uppercase `API` namespace is built into the binary, absent from SQLite and
storage statistics, and listed as `kind: "builtin"` in `/FILES`. Every
mutation below it returns `405` with `Allow: GET, HEAD`.

<!-- contract:api client asset -->
### `GET /{sdk}`

The exact paths `/symbol.ts`, `/symbol.js`, `/symbol.global.js`,
`/symbol.d.ts`, and `/symbol.py` serve generated artifacts embedded in the
binary. The former `/api.*` paths remain byte-identical compatibility aliases. They use strong
ETags, `Cache-Control: no-cache`, and conditional `304` responses.

<!-- contract:api client hash -->
### `GET /{sdk}/HASH`

The same five paths followed by `/HASH` return the lowercase 64-hex Blake3
digest plus a newline.

<!-- contract:api version -->
### `GET /API/VERSION`

Returns `api_version`, `absolute_revision`, `source_hash`, `commit`, and
`dirty` as JSON with a strong ETag and `Cache-Control: no-cache`. `commit` is
the raw hexadecimal Git object name baked into the server, or `unknown`.
`dirty` is true when that build included uncommitted canonical source.

Canonical client: `symbol api`, or `symbol api --json`.

## Protocol conventions

- Site names are 1–63 characters, lowercase ASCII letters/digits with hyphens
  only in the middle. `files`, `stats`, and `symbol` are reserved.
- File paths are slash-separated relative paths. Empty and `..` paths are
  rejected. Backslashes normalize to slashes; empty and `.` components are
  removed.
- `FILES`, `UNDO`, and `EXPIRES` reserve their whole top-level virtual
  namespace: a mutation path whose first component is one of those names is
  rejected, including the name itself and every descendant.
- `FILES`, `HASH`, `UNDO`, `EXPIRES`, `symbol.toml`, `.symbol-token`, and
  `.symbol-claim` are also reserved as the final component of any mutation
  path.
- After managed-site authentication, `POST`, `ALIAS`, `REPLACE`, `PATCH`,
  `PUT`, `DELETE`, and `EXPIRE` apply both rules uniformly and return
  `400 Bad Request` with the frozen plain-text body
  `error: path is reserved by symbol\n`. Authentication failure takes
  precedence and returns `401`.
- Plain responses are UTF-8 text with a trailing newline.
- Symbol never physically deletes persisted blob bytes during startup or
  mutation GC. Unreferenced and replaced-corrupt blobs move to the local
  `.quarantine` tree; startup restores any quarantined hash referenced by the
  catalog. Quarantine removal is an explicit offline operator action only
  after a verified full backup.
- JSON responses use `application/json`.
- Timestamps are RFC 3339. HTTP `Expires` uses an RFC-compatible GMT date.
- File hashes are lowercase, unprefixed 64-hex Blake3 values in blob ETags and
  `/HASH` bodies. Inventory and manifest hashes use `blake3:<64 hex>`.
- Axum automatically serves `HEAD` for `GET` routes. There is no separately
  implemented HEAD-specific contract.
- Unknown methods return `405` only where a route fallback is installed
  (`/{name}`, `/{name}/`, and `/{name}/{path...}`). Other unmatched
  method/path combinations use Axum's default `405` or `404` response.

## Authentication and creation identity

Reads are public. An unmanaged site's mutations are public. Every mutation of
a managed site requires:

```http
Authorization: Bearer sym_mgmt_<64 lowercase hex>
```

Missing or invalid authorization returns:

```http
HTTP/1.1 401 Unauthorized
WWW-Authenticate: Bearer realm="symbol"
Cache-Control: no-store
Content-Type: text/plain; charset=utf-8

error: management token required
```

Management tokens are generated from 32 random bytes, stored only as
domain-separated Blake3-derived hashes, compared in constant time, returned
once, and not recoverable.

At site creation the service records either an authenticated configured
creator principal or a creator claim. Without a trusted principal and without a
caller-supplied claim, it generates a creator claim and returns it once:

```http
Creator-Claim: sym_claim_<64 lowercase hex>
Cache-Control: no-store
```

A caller may instead send `Creator-Claim: sym_claim_...`. To create a managed
site in the same request, send `Management-Action: claim`; a new management
token is returned in `Management-Token`. Creator identity may come from one
configured trusted-proxy account header, a certificate fingerprint supplied by
a trusted TLS terminator after it validates mTLS, a Tailscale user header, or
the direct `tailscale whois --json` resolver configured with
`SYMBOL_TAILSCALE_WHOIS_COMMAND`. Symbol does not terminate TLS or inspect
client certificates itself. Identity headers are accepted only from
peer IPs in `SYMBOL_TRUSTED_PROXY`; caller-supplied internal principal headers
are always stripped. Management audit rows retain that socket peer IP as
non-authoritative context. Peers listed separately in
`SYMBOL_AUDIT_TRUSTED_PROXY` may supply the left-most valid
`X-Forwarded-For` address for audit context; untrusted peers cannot override
their socket address.

## Common mutation request headers

`Authorization`
: Bearer management token. Required only when the current site/name tombstone
  is managed.

`If-Match`
: One quoted or unquoted `blake3:<64 hex>` site tree hash. Accepted on PUT,
  ALIAS, POST, REPLACE, and PATCH.
  Lists and `*` are rejected. A mismatch returns `412` without writing.

`Idempotency-Key`
: 1–256 visible ASCII characters (`0x21`–`0x7e`). Implemented for unnamed
  PUT, generated-destination COPY, management claim/rotate, ALIAS, allocated
  POST workflows, REPLACE, and PATCH.
  Records live four hours. Reuse with the same operation and fingerprint
  replays the stored generated name and mutation metadata; reuse for a
  different request, including a changed `If-Match`, returns `409`. Named PUT,
  named COPY, MOVE, DELETE, EXPIRE, UNDO, status, and release do not implement
  idempotency.

  Replays return `Idempotency-Replayed: true` and the original resource/status
  metadata. They never return or re-expose management or creator secrets.
  Persist one-time credentials from the original response; use the retained
  creator identity/claim recovery flow if that response was lost.

The shell client writes a mode-0600 typed pending-operation record before
retry-sensitive PUT/COPY requests. `symbol recover` replays surviving
idempotency state in a new process, maps generated destinations, and promotes
the pending claim to the final site record. Pending records older than seven
days are removed.

`Management-Action: claim`
: On creation, creates the site as managed and returns a one-time management
  token. Other management action values on creation return `400`.

`Creator-Claim`
: Canonical creator claim used for later claim/rotation recovery.

`Content-Disposition`
: Supplies the logical filename for a root PUT.

`Unpack`
: `1`, `true`, `yes`, or an empty value extracts a supported archive and
  merges its retained files.

`Replace`
: On named site PUT only, `1`, `true`, `yes`, or an empty value replaces the
  site tree and deletes remote paths that are not in the upload. Generated
  `symbol.toml` is kept. File PUT and unnamed PUT reject the header.

## Common mutation response headers

Content mutations return some or all of:

```http
Location: https://symbol.example/hello/
ETag: "blake3:<site tree hash>"
Content-Revision: 2
Undo-Token: <32 lowercase hex>
Undo-Expires: 2026-08-21T01:30:00Z
Sanitized-Management-Tokens: 1
Sanitized-Creator-Claims: 2
Management-Token: sym_mgmt_...
Creator-Claim: sym_claim_...
```

`Undo-Token` is omitted for a no-op mutation. Sanitization count headers are
omitted when zero. Secret-returning responses use `Cache-Control: no-store`.
`Undo-Expires` is the RFC 3339 deadline for the matching undo token.

## Common read headers

`Accept` selects HTML, plain text, or JSON where documented.
`If-None-Match` enables ETag revalidation. `Range` and `If-Range` select one
byte range for stored files and immutable blobs.

Successful file responses may include `Content-Length`, `Content-Range`,
`Accept-Ranges`, `ETag`, `Cache-Control`, and `Expires`.

## Route inventory

<!-- contract:docs -->
### `GET /`

Renders this service's public guide as HTML or plain text. `Accept` explicitly
prefers `text/html`/`application/xhtml+xml` or
`text/plain`/`text/markdown`/`text/x-markdown`; otherwise browser-like user
agents receive HTML and curl/wget/HTTPie receive plain text.

Success: `200`; conditional `If-None-Match`: `304`.

Headers: strong body `ETag`, `Cache-Control: no-cache`,
`Vary: Accept, User-Agent`, and an alternate-representation `Link`.

Canonical client: `symbol help` is local client help; there is no client command
that fetches this page.

<!-- contract:unnamed put -->
### `PUT /`

Publishes to a generated name, normally four lowercase alphanumeric
characters. Body handling is identical to named site PUT. `Idempotency-Key`
applies and should be used to avoid creating a second site after a lost
response.

Success: `201 Created`.

```http
PUT / HTTP/1.1
Host: symbol.example
Content-Type: text/html
Idempotency-Key: deploy-2026-08-20
Content-Length: 14

<h1>Hello</h1>
```

```http
HTTP/1.1 201 Created
Location: https://symbol.example/k7qm/
ETag: "blake3:..."
Content-Revision: 1
Undo-Token: 7d0e...
Undo-Expires: 2026-08-21T01:30:00Z
Creator-Claim: sym_claim_...
Cache-Control: no-store
Content-Type: text/plain; charset=utf-8

ok k7qm https://symbol.example/k7qm/ (1 files, changed: true)
```

Canonical client: `symbol put FILE`, or piped `symbol put -`.

<!-- contract:docs hash -->
### `GET /HASH`

Returns the lowercase Blake3 hash of the rendered plain documentation body.
This endpoint itself does not implement conditional caching and returns
`text/plain`.

Success: `200`.

<!-- contract:installer -->
### `GET /install.sh`

Returns the installer with the configured public URL substituted.

Success: `200`; conditional `If-None-Match`: `304`.

Headers: `Content-Type: text/x-shellscript; charset=utf-8`, body `ETag`,
`Cache-Control: no-cache`.

<!-- contract:installer hash -->
### `GET /install.sh/HASH`

Returns the lowercase Blake3 hash of the compile-time installer template, not
the URL-substituted response.

Success: `200`.

<!-- contract:client -->
### `GET /symbol.sh`

Returns the shell client with the configured public URL substituted.

Success: `200`; conditional `If-None-Match`: `304`.

Headers match `/install.sh`.

Canonical client: `symbol update` downloads this route.

<!-- contract:client hash -->
### `GET /symbol.sh/HASH`

Returns the lowercase Blake3 hash of the compile-time client template, used by
the installer and update check.

Success: `200`.

<!-- contract:stats -->
### `GET /STATS` and `GET /STATS/`

Returns storage and in-process serving metrics. Generated `symbol.toml` files
are excluded from file/blob size statistics.

Success: `200`.

```json
{
  "sites": 2,
  "files": 5,
  "blobs": 4,
  "bytes": 900,
  "logical_bytes": 1200,
  "saved_bytes": 300,
  "saved_fraction": 0.25,
  "file_sizes": {
    "min": 10,
    "p25": 20.0,
    "median": 100.0,
    "mean": 240.0,
    "p75": 300.0,
    "max": 770,
    "iqr": 280.0,
    "stddev": 285.0
  },
  "blob_sizes": {
    "min": 10,
    "p25": 20.0,
    "median": 100.0,
    "mean": 225.0,
    "p75": 300.0,
    "max": 770,
    "iqr": 280.0,
    "stddev": 300.0
  },
  "serving": {
    "cache": {"hits": 10, "misses": 2, "evictions": 0},
    "readers": {
      "operations": 20,
      "waits": 1,
      "wait_micros": 50,
      "query_micros": 300
    }
  }
}
```

Every distribution value is nullable when there are no samples. `sites`,
`files`, `blobs`, and byte/count fields are unsigned integers; distribution
percentiles, mean, IQR, standard deviation, and `saved_fraction` are numbers.

Canonical client: `symbol stats`.

<!-- contract:site listing -->
### `GET /FILES` and `GET /FILES/`

Lists all sites. HTML/plain negotiation follows `/`; exact
`Accept: application/json` (parameters allowed) selects JSON.
Generated `symbol.toml` contributes to these listing counts and logical bytes,
unlike `/STATS`.

```json
{
  "path": "/",
  "files": 3,
  "bytes": 1200,
  "entries": [
    {"kind": "site", "name": "hello", "files": 3, "bytes": 1200}
  ]
}
```

Success: `200`; conditional `If-None-Match`: `304`.

Headers: body `ETag`, `Cache-Control: no-cache`,
`Vary: Accept, User-Agent`.

Canonical client: `symbol ls` / `symbol ls -l`.

<!-- contract:site redirect -->
### `GET /{name}`

For an existing site, returns `307 Temporary Redirect` to `/{name}/`.
If `{name}` ends in `.tar.gz`, `.tar`, or `.zip`, it is instead interpreted as
an archive download name, as described below. Missing sites return `404`.

<!-- contract:site index -->
### `GET /{name}/`

Serves `index.html`, then `index.htm`, when present. Otherwise renders the
directory listing. Nested directories behave the same; a directory URL without
a trailing slash receives `307`.

Files use MIME type inference from their logical path, `ETag: "<raw hash>"`,
`Cache-Control: no-cache`, `Accept-Ranges: bytes`, and
`X-Content-Type-Options: nosniff`. `If-None-Match` supports lists, `*`, and
weak validators and returns `304`.

When an effective expiry exists, responses also include HTTP `Expires`,
`Cache-Control: no-cache`, and `Expiry-Mode` only when the requested target has
its own policy.

<!-- contract:site file -->
<!-- contract:file hash -->
<!-- contract:expiry target -->
### `GET /{name}/{path...}`

Serves a file or directory as above. If the exact path is missing, a
non-directory request transparently tries `{path}.html` and then `{path}.htm`;
an exact extensionless file or directory always wins. Explicit missing
`.html`/`.htm` paths remain `404`.

Unambiguous `.html` / `.htm` files also redirect: `GET /{name}/about.html`
returns `307` to `/{name}/about` when no file, directory, or alias occupies
`about`. A `.htm` file stays put when `{stem}.html` exists, because the
extensionless fallback prefers `.html`. HTML directory listings link to those
same pretty paths, but every listing labels an entry with its stored name,
extension included. An `index.html` entry, or an `index.htm` entry with no
`index.html` beside it, links to the directory that serves it rather than to a
path that would only redirect. `PUT`, `DELETE`, `EXPIRE`, and `HASH` stay on
the real path.

These control suffixes are reserved:

- `/{name}/{path...}/HASH` returns the raw stored file hash with `200`, or `404`
  for a directory/missing path.
- `/{name}/{path}/EXPIRES` returns the expiry report JSON.

`/{name}/HASH` returns the hash of root `index.html` or `index.htm`, preferring
`index.html`; it returns `404` when neither exists.

Canonical client: no direct content command; `symbol url NAME` prints the site
root.

### byte ranges

Stored files support one `Range: bytes=...` range in start-end, start-, or
`-suffix-length` form. A matching `If-Range` ETag permits the range; a
non-matching `If-Range` returns the full `200` representation. Multiple ranges
and a non-`bytes=` unit are ignored and return the full body. Syntactically
invalid or unsatisfiable single byte ranges return:

```http
HTTP/1.1 416 Range Not Satisfiable
Content-Range: bytes */1234
Accept-Ranges: bytes
ETag: "<raw hash>"
Cache-Control: no-cache
```

A successful range returns `206`, `Content-Range`, and the selected
`Content-Length`. Files above 1 MiB and all ranges stream from disk; smaller
complete files may use the 64 MiB/16,384-entry in-process blob cache.

<!-- contract:archive get -->
### `GET /{name}.tar.gz`, `GET /{name}.tar`, `GET /{name}.zip`

Streams a generated archive without changing the site.

Success: `200`.

Headers: format-specific `Content-Type`, `Content-Length`,
`Content-Disposition: attachment; filename="..."`, `Cache-Control: no-cache`,
`X-Content-Type-Options: nosniff`, plus effective expiry headers.

Canonical client: `symbol get NAME [ARCHIVE]`; `symbol clone NAME [DIR]`
downloads tar.gz and extracts it.

<!-- contract:files inventory -->
### `GET /{name}/FILES` and `GET /{name}/FILES/`

Lists the root. Exact `Accept: application/json` selects the sync inventory,
which differs from normal listing JSON:

```http
GET /hello/FILES HTTP/1.1
Accept: application/json
```

```http
HTTP/1.1 200 OK
ETag: "blake3:<tree hash>"
Content-Revision: 4
Cache-Control: no-cache
Content-Type: application/json

{"site":"hello","created_at":"2026-01-02T03:04:05Z","updated_at":"2026-01-02T03:04:05Z","content_revision":4,"tree_hash":"blake3:<tree hash>","events":[{"kind":"created","at":"2026-01-02T03:04:05Z","files":0}],"files":[{"path":"index.html","hash":"blake3:<file hash>","size":14}],"aliases":[]}
```

Inventory files are sorted by path and exclude generated `symbol.toml`.
`created_at` is null for sites created before this field existed. `events` is
the newest-first publish timeline (`created`, `publish`, `rename`, `restore`),
capped at 100 entries, where `files` counts changed paths. GET responses carry
`Last-Modified` from the site's last publish time.
The inventory JSON path does not process `If-None-Match`; it always returns
`200`. Without JSON Accept, this route returns a cached body listing with the
listing schema below.

Canonical client: `symbol ls NAME`; `symbol sync` uses inventory JSON.

<!-- contract:files subtree -->
### `GET /{name}/FILES/{path...}`

Lists a directory. A missing trailing slash on a directory returns `307`; a
file redirects with `307` to its content URL. JSON listing schema:

```json
{
  "path": "hello/assets/",
  "files": 2,
  "bytes": 900,
  "entries": [
    {"kind": "directory", "name": "icons", "files": 1, "bytes": 500},
    {"kind": "file", "name": "logo.svg", "bytes": 400}
  ]
}
```

`kind` is `directory` or `file`. `files` is omitted on file entries. Success,
caching, and content negotiation match `/FILES`.

<!-- contract:site put -->
### `PUT /{name}` and `PUT /{name}/`

Creates or merges into a named site. Uploaded paths replace same-path files;
other existing paths are retained unless `Replace` is set. `Replace` (`1`,
`true`, `yes`, or empty) deletes remote files, aliases, allocated entries, and
non-site expiry that are not in the upload; generated `symbol.toml` is kept.
An identical merge or replace with nothing to prune returns `200`,
`changed: false`, no undo headers, and does not increment the content revision.
A changed existing site returns `200`; a new site returns `201`. Create plus
`Replace` is ordinary put.

Body interpretation:

- `Unpack` enables extraction when its trimmed value is empty, `1`, `true`, or
  `yes` (case-insensitive).
- Input is detected by magic bytes first, then Content-Type, filename
  extension, and HTML prefix.
- Supported extracted formats are zip, tar, tar.gz, and single-file gz.
- Without `Unpack`, an archive is stored as one file.
- `Content-Disposition: ...; filename=...` selects the stored filename.
  Otherwise defaults are `index.html` for detected HTML, `archive.zip`,
  `archive.tar`, `archive.gz`, or `file`.

Raw merge example with optimistic concurrency:

```http
PUT /hello HTTP/1.1
Content-Type: application/gzip
Content-Disposition: attachment; filename="site.tar.gz"
Unpack: 1
Replace: 1
If-Match: "blake3:<previous tree hash>"
Authorization: Bearer sym_mgmt_...
Content-Length: 1234

<gzip bytes>
```

```http
HTTP/1.1 200 OK
Location: https://symbol.example/hello/
ETag: "blake3:<new tree hash>"
Content-Revision: 5
Undo-Token: 7d0e...
Undo-Expires: 2026-08-21T01:30:00Z
Content-Type: text/plain; charset=utf-8

ok hello https://symbol.example/hello/ (3 files, changed: true)
```

Canonical client: `symbol put NAME SOURCE` or `symbol put --replace NAME SOURCE`;
bare `symbol put` uses the nearest manifest target.

<!-- contract:file put -->
### `PUT /{name}/{path...}`

Creates or replaces one file at the exact path. It does not honor `Unpack` or
`Replace` and does not implement `Idempotency-Key`. It accepts `If-Match`.

Success: `201` only when the site itself is created; otherwise `200`, including
when a new path is added to an existing site. The response body is
`ok /{name}/{path} (changed: true|false)`.

Canonical client: `symbol put NAME FILE DEST`, or
`symbol put NAME FILE` where the client derives a destination.

### upload limits and normalization

- Empty bodies return `400`.
- Stored file requests use `SYMBOL_MAX_FILE_SIZE`, default 4 GiB.
- Requests with `Unpack` use `SYMBOL_MAX_ARCHIVE_UPLOAD`, default 50 MiB.
- Extraction uses `SYMBOL_MAX_ARCHIVE_EXTRACTED` (default 80 MiB) and
  `SYMBOL_MAX_ARCHIVE_FILES` (default 5000). Limit errors return
  `413 Payload Too Large`.
- Archive entries must be regular files. Unsafe traversal paths fail.
- A single common archive root directory is stripped.
- Known OS metadata (`__MACOSX`, AppleDouble, `.DS_Store`, Windows thumbnail
  metadata, and related entries) and Apple resource-fork payloads are dropped.
- An archive containing no retained files returns `400`.
- Uploaded `symbol.toml` is ignored; the server regenerates it.

### sanitization

Canonical `sym_mgmt_` and `sym_claim_` tokens have a 64-character lowercase
hex payload. Every file up to and including 1 MiB is scanned and redacted.
Larger files are scanned only when they are valid UTF-8 text without NULs and
with no more than 1% suspicious ASCII controls. Redaction preserves the prefix
and byte length, replacing the 64-byte payload with `*`.

Large binary files are not scanned. A supported archive uploaded without
`Unpack` is inspected through at most `SYMBOL_MAX_ARCHIVE_EXTRACTED` bytes and
rejected with `400` if a token is found. Sanitization is therefore a narrow
token leak barrier, not general secret detection.

Allocated uploads, replacement bodies, and completed splice results use the
same redaction before content hashing and content-addressed naming. Their
headers and JSON receipts report the sanitized token counts.

<!-- contract:file delete -->
### `DELETE /{name}/{path...}`

Deletes one file or a directory subtree and its descendant expiry policies.

Success: `200` with `deleted {name}/{path}` and undo headers. Missing paths
return `404`.

Canonical client: `symbol rm NAME PATH`.

<!-- contract:site pop -->
<!-- contract:archive pop -->
### `DELETE /{name}` and `DELETE /{name}/`

Removes a site and streams a tar.gz snapshot in the response. The route also
recognizes `.tar.gz`, `.tar`, and `.zip` suffixes, selecting that response
archive format.

Success: `200` with `Content-Type`, `Content-Length`,
`Content-Disposition`, `Undo-Token`, and `Undo-Expires`.

```sh
curl -D headers.txt -o hello.tar.gz -X DELETE \
  "$SYMBOL_BASE/hello.tar.gz"
```

Canonical client: `symbol pop NAME [ARCHIVE]`. `symbol rm NAME` also calls this
route but discards the archive body, prints `deleted NAME`, and retains the undo
hint. Use `pop` when the archive is wanted and `rm` for deletion-only output.

<!-- contract:site copy -->
### `COPY /{name}` and `COPY /{name}/`

Copies a site, excluding and regenerating `symbol.toml`.

`Destination: /new-name` selects a name and must identify exactly one valid
site. An absolute URI is accepted but only its path is used. Without
`Destination`, a name is generated and `Idempotency-Key` applies. With an
explicit destination, idempotency is not implemented.

Success: `201`, mutation headers, and body:

```text
ok new-name https://symbol.example/new-name/ (3 files)
```

The copy is public/unmanaged unless creation includes
`Management-Action: claim`. Source management does not transfer. Expiry
policies transfer with newly calculated relative/decay deadlines. The copy has
an undo entry that removes it.

Canonical client: `symbol copy [--managed] SRC [DST]`; `symbol remix` adds a
local clone.

<!-- contract:site move -->
### `MOVE /{name}` and `MOVE /{name}/`

Renames a site. `Destination` is required and the destination must not exist.
Managed state and management token remain attached to the site.

Success: `200`, `Location`, tree/revision and undo headers, body:

```text
moved https://symbol.example/old/ -> https://symbol.example/new/
```

Canonical client: `symbol move SRC DST`.

<!-- contract:undo stack -->
### `GET /{name}/UNDO` and `GET /{name}/UNDO/`

Returns unconsumed, unexpired undo records newest first.

Success: `200`, `Cache-Control: no-cache`.

```json
{
  "site": "hello",
  "entries": [
    {
      "token": "7d0e...",
      "kind": "put",
      "description": "restore previous state of hello",
      "created_at": "2026-08-20T21:30:00Z",
      "expires_at": "2026-08-21T01:30:00Z",
      "remaining_seconds": 14399
    }
  ]
}
```

`kind` is `put`, `delete_path`, `delete_site`, `copy`, `move`, `expiry`, or
`expire_sweep`.

Canonical client: `symbol undo --stack NAME`.

<!-- contract:site undo -->
### `UNDO /{name}` and `UNDO /{name}/`

Restores the newest undo record associated with the current or former site
name. `Undo-Token` is an optional compare-and-swap guard. If supplied and not
the newest token, the server returns `409` and names the latest token.

Success: `200`:

```text
restored hello to 2026-08-20T21:30:00Z
```

Undo is LIFO, one-shot, retained for four hours, and capped at ten records per
site. It restores content and expiry state. Move undo recognizes both names.
Managed-site deletion retains a management tombstone, so recreating or undoing
that name still requires the previous management token; restored content
remains managed.

Canonical client: `symbol undo [NAME [TOKEN]]`.

<!-- contract:expiry inventory -->
### `GET /{name}/EXPIRES`, `GET /{name}/EXPIRES/`, and
`GET /{name}/{path...}/EXPIRES`

The site-level route returns:

```json
{"site":"hello","entries":[{"target":{"site":"hello","path":null,"kind":"site"},"size":42,"refreshed_at":"2026-08-21T00:00:00Z","own_policy":{"mode":"relative","min_age_seconds":null,"max_age_seconds":null,"max_size_bytes":null,"power":null,"retention_seconds":3600,"expires_at":"2026-08-21T01:00:00Z"},"inherited_caps":[],"effective_expires_at":"2026-08-21T01:00:00Z","remaining_seconds":3600,"limited_by":null}]}
```

`entries` contains every stored site, folder, and file policy. The path-level
route returns one effective expiry report for that target. No authentication
is required.

Success: `200`, `Cache-Control: no-cache`.

```json
{
  "target": {"site": "hello", "path": "assets", "kind": "folder"},
  "size": 1048576,
  "refreshed_at": "2026-08-20T21:30:00Z",
  "own_policy": {
    "mode": "decay",
    "min_age_seconds": 2592000,
    "max_age_seconds": 31536000,
    "max_size_bytes": 536870912,
    "power": 3.0,
    "retention_seconds": 31366737,
    "expires_at": "2027-08-18T..."
  },
  "inherited_caps": [
    {"kind": "site", "path": null, "expires_at": "2026-09-01T00:00:00Z"}
  ],
  "effective_expires_at": "2026-09-01T00:00:00Z",
  "remaining_seconds": 960000,
  "limited_by": {"kind": "site", "path": null}
}
```

`target.kind`, inherited `kind`, and limit `kind` are `site`, `folder`, or
`file`. With expiration disabled, `refreshed_at`, `own_policy`,
`effective_expires_at`, `remaining_seconds`, and `limited_by` are null and
`inherited_caps` is empty. For relative/absolute policies, decay-only fields
are null. Absolute policy `retention_seconds` is null.

Canonical client: `symbol expire NAME [PATH] --show`.

<!-- contract:site expire -->
<!-- contract:file expire -->
### `EXPIRE /{name}[/{path...}]`

Sets or removes the exact target's policy and returns the report schema above.
Folder/site policies cap descendants; the earliest effective deadline wins.

Headers select exactly one mode:

- no `Expiry-Mode` and no expiry parameter headers: server default decay;
- `Expiry-Mode: never`: remove the exact policy;
- `Expiry-Mode: relative` plus `Expiry-In: <positive integer><s|m|h|d|w>`;
- `Expiry-Mode: absolute` plus `Expiry-At: <RFC3339 timestamp with offset>`;
- `Expiry-Mode: decay`, optionally with `Expiry-Min-Age`,
  `Expiry-Max-Age`, `Expiry-Max-Size`, and `Expiry-Power`.

Decay size units are `B`, decimal `KB` through `EB`, or binary `KiB` through
`EiB`. Min age must not exceed max age; max size and finite power must be
positive.

The default curve is:

```text
retention = min_age + (max_age - min_age)
            * (1 - min(size, max_size) / max_size)^power
```

Defaults are 30 days, 365 days, 512 MiB, and power 3 unless changed by server
configuration. On that cubic default curve, the 256 MiB midpoint retains
exactly 71.875 days. Relative and decay deadlines refresh when a mutation
changes a file in their target; their size is recalculated. Absolute deadlines
never refresh. COPY recreates policies at copy time; MOVE preserves them.

Success: `200`, `Cache-Control: no-cache`, expiry headers, and undo headers when
the policy state changed. Removing an already absent policy has no undo entry.

The expiry worker checks at least once per minute and deletes due site/file/
folder targets. It snapshots once per affected site so expiry remains undoable
for four hours.

Canonical client: `symbol expire NAME [PATH] [POLICY]`.

<!-- contract:site management -->
### `MANAGE /{name}` and `MANAGE /{name}/`

Requires one `Management-Action` value:

`status`
: Returns `{"managed":false}` or `{"managed":true}`. No token required.

`claim`
: Converts an unmanaged site to managed. Authorization requires either the
  matching creator claim or matching configured creator principal. Returns
  `{"managed":true}` and a one-time `Management-Token`.

`rotate`
: Replaces the token. Authorization may be the current Bearer token, matching
  creator claim, or matching trusted principal. Returns a one-time token.

`release`
: Requires the current Bearer token, removes management protection and its
  tombstone, and returns `{"managed":false}`.

All success responses are `200` with `Cache-Control: no-store`. Claim and
rotate support `Idempotency-Key` for four hours. A replay returns no token
(because tokens are one-time) and adds `Idempotency-Replayed: true`; callers
must retain the token from the original response. Authorization is checked
before replay, so an old Bearer token cannot authenticate a retry after a
bearer-only rotation; a matching creator claim or trusted principal remains
valid.

Canonical client:
`symbol manage NAME --claim|--status|--rotate|--release`.

<!-- contract:alias batch -->
<!-- contract:alias file -->
### `ALIAS /{name}/` and `ALIAS /{name}/{path...}`

Creates or retargets live aliases without dereferencing them. A single-path
request uses `Alias-Target`; the root batch form uses
`Content-Type: application/json` and
`{"aliases":[{"path":"current","target":"assets/app.js"}]}`. Both accept
`Authorization`, `If-Match`, and `Idempotency-Key`. Targets are site-relative;
absolute/external targets, root escapes, prefix shadowing, and cycles fail
atomically. Batch limits are 4096 entries, 1 MiB JSON, and 4096 bytes per
target.

Success is `201` when every changed row was newly created, otherwise `200`.
Responses are JSON and include `Location`, `ETag`, `Content-Revision`,
optional `Undo-Token`/`Undo-Expires`, and `Idempotency-Replayed` on exact
replay. Stored receipt rows are replayed unchanged even if an alias is later
retargeted.

```http
ALIAS /hello/current HTTP/1.1
Alias-Target: assets/app.js
If-Match: "blake3:<tree hash>"
Idempotency-Key: alias-current-v1
```

<!-- contract:allocated file -->
### `POST /{name}/{folder...}/`

Creates a content-addressed file under a server-selected name. The default
action (`Allocation-Action: create`) accepts `File-Prefix`, `File-Suffix`,
`File-Extension`, `Content-Type`, expiry headers (`Expiry-Mode`, `Expiry-In`,
`Expiry-At`, `Expiry-Min-Age`, `Expiry-Max-Age`, `Expiry-Max-Size`,
`Expiry-Power`), `Authorization`, `If-Match`, and `Idempotency-Key`.

Two-phase custom naming uses `Allocation-Action: propose` with the body and an
optional `File-Extension`, then `Allocation-Action: finalize` with an empty
body, `Allocation-Token`, and `File-Name`. `Allocation-Action: cancel` uses an
empty body and `Allocation-Token`. Proposal hints, media type, expiry,
authorization identity, and `If-Match` are bound to idempotency. Cancellation
performs its tree CAS and pending-row deletion in one transaction.

Create/finalize return `200` or `201`; propose returns `202`; cancel returns
`200`. JSON responses include `Location`, `ETag`, `Content-Revision`, and,
where applicable, `Content-Location`, `Undo-Token`, `Undo-Expires`, and
`Idempotency-Replayed`.

```sh
curl -X POST -H 'Content-Type: image/png' \
  -H 'File-Prefix: avatar-' -H 'File-Extension: png' \
  -H 'Idempotency-Key: avatar-v1' --data-binary @avatar.png \
  "$SYMBOL_BASE/hello/uploads/"
```

<!-- contract:file replace -->
### `REPLACE /{name}/{path...}`

Atomically replaces a regular or allocated file. `If-Content-Match` supplies
the required raw or `blake3:` content hash; `If-Match` guards the site tree.
`Content-Type`, `Authorization`, and `Idempotency-Key` are accepted. Allocated
files may relocate according to their stored naming rule. Success is `200`
JSON with `Location`, `ETag`, `Content-Revision`, optional
`Undo-Token`/`Undo-Expires`, and `Idempotency-Replayed`.

```http
REPLACE /hello/data.bin HTTP/1.1
If-Content-Match: blake3:<file hash>
If-Match: "blake3:<tree hash>"
Content-Type: application/octet-stream
Idempotency-Key: replace-data-v2

<replacement bytes>
```

<!-- contract:file splice -->
### `PATCH /{name}/{path...}`

Applies ordered byte splices with `If-Content-Match`, optional `If-Match`,
`Authorization`, and `Idempotency-Key`. The header form uses one or more
`Splice: offset=<n>; delete=<n>; insert=<n>` descriptors, separated from one
another by commas, followed by the exact concatenated insertion bytes (at most
64 descriptors and 8192 aggregate header bytes). Binary frame v1 uses
case-insensitive media type
`Content-Type: application/vnd.symbol.splice; version=1`, magic `SYMSPL1\0`,
big-endian count/flags/descriptor table, at most 4096 descriptors and 96 KiB
metadata, then exact insertion bytes with no trailing data.

```http
PATCH /hello/data.bin HTTP/1.1
If-Content-Match: blake3:<file hash>
Splice: offset=0; delete=0; insert=1
Idempotency-Key: splice-data-v1
Content-Length: 1

X
```

Success is `200` JSON with `Location`, `ETag`, `Content-Revision`, optional
`Undo-Token`/`Undo-Expires`, and `Idempotency-Replayed`. Malformed/range/size
errors are `400`, `416`, or `413`; stale content or tree guards return `412`.

## Generated `symbol.toml`

Every site contains a server-generated `symbol.toml` and archives include it.
Uploads cannot replace it. It is excluded from inventory/stats file counts and
from the tree hash.

The tree hash covers site content only: stored files, allocated entries, and
aliases. Expiry policies are listed in this manifest but are deliberately
outside the hash, so an `EXPIRE` that changes only a policy rewrites the
manifest while leaving `tree_hash`, `content_revision`, and therefore the site
ETag unchanged; an `If-Match` captured before it still applies. This is the
same boundary `GET /{name}/FILES` inventory JSON draws by excluding the
generated manifest, so the ETag covers exactly what the inventory reports.

Example:

```toml
version = 1
host = "https://symbol.example"
name = "hello"
managed = false
content_revision = 4
tree_hash = "blake3:<64 hex>"

[files]
"index.html" = "blake3:<64 hex>"

[expiry.site]
mode = "relative"
expires_at = "2026-09-01T00:00:00Z"
duration_seconds = 604800
```

Folder policies use `[expiry.folders."<path>"]`; file policies use
`[expiry.files."<path>"]`. Decay policies include `min_age_seconds`,
`max_age_seconds`, `max_size_bytes`, and `power`; absolute policies contain
the mode and computed `expires_at`.

The canonical client preserves local top-level `token` and `claim` keys when it
refreshes the server manifest. Those two local keys are client configuration,
not generated server fields.

`symbol sync` verifies both the manifest baseline tree hash and file map
against `GET /{name}/FILES` JSON, computes local Blake3 hashes, ignores local
deletions, and PUTs only additions/modifications as an unpacked archive with
`If-Match`. The conditional write is atomic. There is no server-side
three-way merge or deletion manifest.

## Status and error mapping

- `200 OK`: successful reads, existing-site/no-op PUT, DELETE, MOVE, UNDO,
  EXPIRE, and MANAGE.
- `201 Created`: new-site PUT and COPY.
- `206 Partial Content`: valid byte range.
- `304 Not Modified`: body-cache and file ETag validation.
- `307 Temporary Redirect`: canonical site/directory/content URL.
- `400 Bad Request`: invalid names/paths/headers, empty/invalid archives,
  unsupported extraction, reserved path, invalid expiry/idempotency input,
  malformed tokens, and most domain validation errors.
- `401 Unauthorized`: managed mutation missing/wrong Bearer token, or rotate
  without any accepted credential.
- `403 Forbidden`: management claim lacks its creator identity/claim, or
  rotation targets an unmanaged site.
- `404 Not Found`: missing site/path/hash/undo.
- `409 Conflict`: stale undo token, destination exists, idempotency conflict,
  reserved-path migration collision, already managed, or creation management
  requested against an existing site.
- `412 Precondition Failed`: `If-Match` tree mismatch. Response includes the
  current `ETag` and `Content-Revision`; nothing is written.
- `413 Payload Too Large`: file/compressed/extracted/file-count limits.
- `416 Range Not Satisfiable`: invalid or unsatisfiable byte range.
- `500 Internal Server Error`: SQLite, filesystem, or OS random-source error.

Error bodies are plain implementation messages such as:

```text
error: site not found
error: destination site already exists
error: upstream changed; nothing was written
error: idempotency key was already used for a different request
```

There is no versioned error JSON schema.

## Cache behavior

- Generated docs/scripts/listings and normal files: strong ETag,
  `Cache-Control: no-cache`; matching `If-None-Match` returns `304`.
- Immutable blob route: `Cache-Control: public, max-age=31536000, immutable`.
- Inventory, undo, expiry, archives: `no-cache`; inventory does not implement
  a `304` path despite returning a tree ETag.
- Management and one-time-secret responses: `no-store`.
- Content with effective expiry forces `no-cache` and includes `Expires`.

<!-- contract:immutable blob -->
### `GET /.blob/{name}/{hash}`

Serves raw blob bytes only if the named site currently references that exact
raw hash. It uses `application/octet-stream`, quoted raw-hash ETag, byte ranges,
`nosniff`, and one-year immutable caching. A valid hash not referenced by that
site returns `404`.

This route is public and content-addressed but site-scoped; it is not currently
emitted in generated manifests or mapped by a client command.

## Behavior not implemented

The current service does **not** implement:

- a versioned `/api` namespace, OpenAPI endpoint, or JSON error envelope;
- access-controlled reads or private sites;
- listing pagination, search, quotas, or per-site upload limits;
- WebDAV PROPFIND, multi-range responses, resumable multipart upload sessions,
  or server-side multipart upload;
- automatic remote deletion from `symbol sync`;
- server-side conflict merging;
- recovery of a lost management token through a public HTTP admin endpoint;
- idempotency for named PUT, named COPY, MOVE, DELETE, EXPIRE, UNDO, management
  status, or management release;
- conditional `If-None-Match` handling for inventory JSON, stats, hash
  endpoints, expiry reports, undo stacks, or management status;
- gzip response compression in the application itself (the sample Caddy proxy
  supplies it).
<!-- API:PROTOCOL:END -->

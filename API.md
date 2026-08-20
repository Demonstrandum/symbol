# symbol HTTP API

This is the implementation reference for the current public HTTP surface.
Examples use `https://symbol.example`; substitute the configured
`SYMBOL_PUBLIC_URL`. The human-facing client guide is
[`static/docs.md`](static/docs.md), and deployment details are in
[`README.md`](README.md).

## Protocol conventions

- Site names are 1–63 characters, lowercase ASCII letters/digits with hyphens
  only in the middle. `files`, `stats`, and `symbol` are reserved.
- File paths are slash-separated relative paths. Empty and `..` paths are
  rejected. Backslashes normalize to slashes; empty and `.` components are
  removed.
- `FILES`, `HASH`, `UNDO`, `EXPIRES`, `symbol.toml`, `.symbol-token`, and
  `.symbol-claim` are reserved as the final component of any uploaded or
  deleted path.
- Plain responses are UTF-8 text with a trailing newline.
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

At site creation the service records either an authenticated trusted-proxy
principal or a creator claim. Without a trusted principal and without a
caller-supplied claim, it generates a creator claim and returns it once:

```http
Creator-Claim: sym_claim_<64 lowercase hex>
Cache-Control: no-store
```

A caller may instead send `Creator-Claim: sym_claim_...`. To create a managed
site in the same request, send `Management-Action: claim`; a new management
token is returned in `Management-Token`. The trusted-proxy mode accepts a
principal header only from IPs configured in `SYMBOL_TRUSTED_PROXY`; a
caller-supplied internal principal header is always stripped.

## Common mutation request headers

`Authorization`
: Bearer management token. Required only when the current site/name tombstone
  is managed.

`If-Match`
: One quoted or unquoted `blake3:<64 hex>` site tree hash. Accepted on PUT.
  Lists and `*` are rejected. A mismatch returns `412` without writing.

`Idempotency-Key`
: 1–256 visible ASCII characters (`0x21`–`0x7e`). Implemented only for unnamed
  PUT, generated-destination COPY, management claim, and management rotate.
  Records live four hours. Reuse with the same operation and fingerprint
  replays the stored generated name and mutation metadata; reuse for a
  different request returns `409`. Named PUT, named COPY, MOVE, DELETE, EXPIRE,
  UNDO, status, and release do not implement idempotency.

  Replays return `Idempotency-Replayed: true` and the original resource/status
  metadata. They never return or re-expose management or creator secrets.
  Persist one-time credentials from the original response; use the retained
  creator identity/claim recovery flow if that response was lost.

`Management-Action: claim`
: On creation, creates the site as managed and returns a one-time management
  token. Other management action values on creation return `400`.

`Creator-Claim`
: Canonical creator claim used for later claim/rotation recovery.

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

## Route inventory

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

Canonical client: `symbol put FILE`, or piped `symbol put`.

### `GET /HASH`

Returns the lowercase Blake3 hash of the rendered plain documentation body.
This endpoint itself does not implement conditional caching and returns
`text/plain`.

Success: `200`.

### `GET /install.sh`

Returns the installer with the configured public URL substituted.

Success: `200`; conditional `If-None-Match`: `304`.

Headers: `Content-Type: text/x-shellscript; charset=utf-8`, body `ETag`,
`Cache-Control: no-cache`.

### `GET /install.sh/HASH`

Returns the lowercase Blake3 hash of the compile-time installer template, not
the URL-substituted response.

Success: `200`.

### `GET /symbol.sh`

Returns the shell client with the configured public URL substituted.

Success: `200`; conditional `If-None-Match`: `304`.

Headers match `/install.sh`.

Canonical client: `symbol update` downloads this route.

### `GET /symbol.sh/HASH`

Returns the lowercase Blake3 hash of the compile-time client template, used by
the installer and update check.

Success: `200`.

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

### `GET /{name}`

For an existing site, returns `307 Temporary Redirect` to `/{name}/`.
If `{name}` ends in `.tar.gz`, `.tar`, or `.zip`, it is instead interpreted as
an archive download name, as described below. Missing sites return `404`.

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

### `GET /{name}/{path...}`

Serves a file or directory as above, except these control suffixes:

- `/{name}/{path}/HASH` returns the raw stored file hash with `200`, or `404`
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

### `GET /{name}.tar.gz`, `GET /{name}.tar`, `GET /{name}.zip`

Streams a generated archive without changing the site.

Success: `200`.

Headers: format-specific `Content-Type`, `Content-Length`,
`Content-Disposition: attachment; filename="..."`, `Cache-Control: no-cache`,
`X-Content-Type-Options: nosniff`, plus effective expiry headers.

Canonical client: `symbol get NAME [ARCHIVE]`; `symbol clone NAME [DIR]`
downloads tar.gz and extracts it.

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

{"site":"hello","content_revision":4,"tree_hash":"blake3:<tree hash>","files":[{"path":"index.html","hash":"blake3:<file hash>","size":14}]}
```

Inventory files are sorted by path and exclude generated `symbol.toml`.
The inventory JSON path does not process `If-None-Match`; it always returns
`200`. Without JSON Accept, this route returns a cached body listing with the
listing schema below.

Canonical client: `symbol ls NAME`; `symbol sync` uses inventory JSON.

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

### `PUT /{name}` and `PUT /{name}/`

Creates or merges into a named site. Uploaded paths replace same-path files;
other existing paths are retained. An identical merge returns `200`,
`changed: false`, no undo headers, and does not increment the content revision.
A changed existing site returns `200`; a new site returns `201`.

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

Canonical client: `symbol put NAME SOURCE`; bare `symbol put` uses the nearest
manifest target.

### `PUT /{name}/{path...}`

Creates or replaces one file at the exact path. It does not honor `Unpack` and
does not implement `Idempotency-Key`. It accepts `If-Match`.

Success: `201` only when the site itself is created; otherwise `200`, including
when a new path is added to an existing site. The response body is
`ok /{name}/{path} (changed: true|false)`.

Canonical client: `symbol put NAME FILE DEST`, or
`symbol put NAME FILE` where the client derives a destination.

### upload limits and normalization

- Empty bodies return `400`.
- Stored file requests use `SYMBOL_MAX_FILE_SIZE`, default 4 GiB.
- Requests with `Unpack` have a fixed 50 MiB compressed-body limit.
- Extraction permits at most 80 MiB of retained uncompressed bytes and 5000
  files. Limit errors return `413 Payload Too Large`.
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
`Unpack` is inspected through at most 80 MiB of uncompressed data and rejected
with `400` if a token is found. Sanitization is therefore a narrow token leak
barrier, not general secret detection.

### `DELETE /{name}/{path...}`

Deletes one file or a directory subtree and its descendant expiry policies.

Success: `200` with `deleted {name}/{path}` and undo headers. Missing paths
return `404`.

Canonical client: `symbol rm NAME PATH`.

### `DELETE /{name}` and `DELETE /{name}/`

Removes a site and streams a tar.gz snapshot in the response. The route also
recognizes `.tar.gz`, `.tar`, and `.zip` suffixes, selecting that response
archive format.

Success: `200`, archive headers, and undo headers.

```sh
curl -D headers.txt -o hello.tar.gz -X DELETE \
  https://symbol.example/hello.tar.gz
```

Canonical client: `symbol pop NAME [ARCHIVE]`. `symbol rm NAME` also calls this
route but discards the archive body, prints `deleted NAME`, and retains the undo
hint. Use `pop` when the archive is wanted and `rm` for deletion-only output.

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

### `MOVE /{name}` and `MOVE /{name}/`

Renames a site. `Destination` is required and the destination must not exist.
Managed state and management token remain attached to the site.

Success: `200`, `Location`, tree/revision and undo headers, body:

```text
moved https://symbol.example/old/ -> https://symbol.example/new/
```

Canonical client: `symbol move SRC DST`.

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

### `GET /{name}/EXPIRES`, `GET /{name}/EXPIRES/`, and
`GET /{name}/{path...}/EXPIRES`

Returns the effective expiry report. No authentication is required.

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

### `EXPIRE /{name}` and `EXPIRE /{name}/{path...}`

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

### `MANAGE /{name}` and `MANAGE /{name}/`

Requires one `Management-Action` value:

`status`
: Returns `{"managed":false}` or `{"managed":true}`. No token required.

`claim`
: Converts an unmanaged site to managed. Authorization requires either the
  matching creator claim or a matching trusted-proxy principal. Returns
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

## Generated `symbol.toml`

Every site contains a server-generated `symbol.toml` and archives include it.
Uploads cannot replace it. It is excluded from inventory/stats file counts and
from the tree hash. Example:

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
- PATCH, WebDAV PROPFIND, multi-range responses, resumable upload sessions, or
  server-side multipart upload;
- replacement semantics for directory/archive PUT (PUT is merge-only);
- automatic remote deletion from `symbol sync`;
- server-side conflict merging;
- recovery of a lost management token through a public HTTP admin endpoint;
- idempotency for named writes or lifecycle methods other than generated COPY,
  claim, and rotate;
- conditional `If-None-Match` handling for inventory JSON, stats, hash
  endpoints, expiry reports, undo stacks, or management status;
- gzip response compression in the application itself (the sample Caddy proxy
  supplies it);
- a public `/API.md` HTTP route. This file is repository documentation.

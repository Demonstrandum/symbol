# symbol

Tiny static-site and media hosting for a tailnet.

The public user guide is [`static/docs.md`](static/docs.md) and is served at
`/`. The implementation-grade HTTP contract is [`API.md`](API.md). This
README is for building and operating the service.

## Build

The crate requires Rust 1.98 or newer.

```sh
cargo build --release --locked
```

With Nix:

```sh
nix build
nix develop
```

The documentation, client, installer, and CSS are compiled into the binary.

## Run

```sh
SYMBOL_PUBLIC_URL=https://symbol.example \
  cargo run --release --locked -- \
  --bind 127.0.0.1:4340 --root ./data
```

`SYMBOL_PUBLIC_URL` is required in normal server mode and must be one HTTP(S)
origin with no trailing slash or path. Development may instead set
`SYMBOL_ALLOW_DEV_ORIGIN=true`, which uses `http://symbol`.

CLI flags have equivalent environment variables:

- `SYMBOL_BIND` (default `127.0.0.1:4340`)
- `SYMBOL_ROOT` (default `/var/lib/symbol`)
- `SYMBOL_MAX_FILE_SIZE` in bytes (default 4 GiB)
- `SYMBOL_PUBLIC_URL`
- `SYMBOL_ALLOW_DEV_ORIGIN` (default false)
- `SYMBOL_EXPIRY_MIN_AGE` (default `30d`)
- `SYMBOL_EXPIRY_MAX_AGE` (default `365d`)
- `SYMBOL_EXPIRY_MAX_SIZE` (default `512MiB`)
- `SYMBOL_EXPIRY_POWER` (default `3`)
- `SYMBOL_TRUSTED_PROXY_PRINCIPAL_HEADER` and comma-separated
  `SYMBOL_TRUSTED_PROXY`; these must be configured together

Archive unpacking has fixed limits of 50 MiB compressed, 80 MiB extracted,
and 5000 files.

## Storage and migrations

SQLite stores sites, paths, hashes, sizes, and other metadata in
`SYMBOL_ROOT/symbol.db`. Immutable, Blake3-addressed payloads are stored under
`SYMBOL_ROOT/blobs/`; transient uploads use `SYMBOL_ROOT/tmp/`.

Startup enables WAL, normal synchronous mode, foreign keys, and a five-second
SQLite busy timeout. Schema migrations run automatically through schema
version 4, followed by an integrity check. Startup also:

- migrates legacy inline SQLite BLOBs to external blob files and verifies
  their size and Blake3 hash;
- imports the legacy catalog when present;
- removes known OS metadata, rebuilds generated `symbol.toml` manifests,
  sweeps expired targets, prunes undo/idempotency records, and removes
  unreferenced blob files.

Migration deliberately fails on unsupported schema versions, failed integrity,
or pre-existing paths that now collide with reserved control names. Back up
before upgrading; there is no downgrade migration.

## Backup

Back up the complete `SYMBOL_ROOT`, not only `symbol.db`. For a directly
copyable, consistent snapshot, stop the service and copy the entire directory.

## Operations

Example systemd and Caddy units are in [`ops/`](ops/). The checked-in service
binds only to loopback and Caddy reverse-proxies the public origins. Build the
release binary before starting it, then adapt user, group, paths, hostnames,
and public URL to the deployment:

```sh
sudo cp ops/symbol.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now symbol
sudo systemctl status symbol
```

The service handles SIGINT/SIGTERM gracefully and logs through `tracing`;
`RUST_LOG` controls filtering.

Managed-site recovery is available to an operator without an HTTP admin route:

```sh
symbol --root /var/lib/symbol admin claim SITE
symbol --root /var/lib/symbol admin rotate SITE --token sym_mgmt_...
```

Both commands print a replacement token once. Protect terminal history and
captured output. If trusted-proxy identity is enabled, only configured peer IPs
may supply the configured principal header; all caller-supplied internal
identity headers are removed. The sample Caddyfile does not enable or inject
identity, so management relies on claim/management tokens by default.

## Development

```sh
./check
```

This runs formatting checks, strict Clippy lints, Rust tests, shell syntax
checks, and client conformance tests. The public guide is parsed and rendered
by the Rust page tests.

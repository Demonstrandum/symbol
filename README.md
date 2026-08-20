# symbol

Tiny static-site and media hosting for a tailnet.

The public user guide is [`static/docs.md`](static/docs.md) and is served at
`/`. This README is for building and operating the service.

## Run

```sh
cargo run --release -- \
  --bind 127.0.0.1:4340 \
  --root /var/lib/symbol
```

Equivalent environment variables are `SYMBOL_BIND` and `SYMBOL_ROOT`.
`SYMBOL_MAX_FILE_SIZE` controls the maximum stored-file upload size in bytes
(default: 4 GiB). Archive uploads remain limited to 50 MiB compressed,
80 MiB extracted, and 5000 files.

The deployment units and Caddy configuration live under [`ops/`](ops/).

## Storage

SQLite stores sites, paths, hashes, sizes, and other metadata in
`SYMBOL_ROOT/symbol.db`. Immutable, Blake3-addressed payloads are stored under
`SYMBOL_ROOT/blobs/`.

Older databases containing inline BLOB payloads are migrated automatically on
startup. Blob writes use temporary files and atomic renames; unreferenced files
are cleaned up after metadata commits and during startup recovery.

## Backup

Back up the complete `SYMBOL_ROOT`, not only `symbol.db`. For a directly
copyable, consistent snapshot, stop the service and copy the entire directory.

## Development

```sh
./check
```

This runs formatting checks, strict Clippy lints, and the complete test suite.

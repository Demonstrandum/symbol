PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sites (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    updated INTEGER NOT NULL,
    public_url TEXT NOT NULL DEFAULT '',
    content_revision INTEGER NOT NULL DEFAULT 0,
    tree_hash TEXT NOT NULL DEFAULT '',
    creator_kind INTEGER,
    creator_hash BLOB,
    claim_hash BLOB,
    management_hash BLOB,
    management_status INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS blobs (
    hash TEXT PRIMARY KEY,
    bytes BLOB NOT NULL DEFAULT X'',
    size INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    hash TEXT NOT NULL REFERENCES blobs(hash),
    size INTEGER NOT NULL,
    PRIMARY KEY (site_id, path)
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS undo_operations (
    token TEXT PRIMARY KEY,
    kind INTEGER NOT NULL,
    description TEXT NOT NULL,
    created INTEGER NOT NULL,
    expires INTEGER NOT NULL,
    consumed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS undo_names (
    token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
    name TEXT NOT NULL,
    PRIMARY KEY (token, name)
);

CREATE TABLE IF NOT EXISTS undo_sites (
    token TEXT PRIMARY KEY REFERENCES undo_operations(token) ON DELETE CASCADE,
    name TEXT NOT NULL,
    existed INTEGER NOT NULL,
    public_url TEXT NOT NULL,
    updated INTEGER NOT NULL,
    content_revision INTEGER NOT NULL,
    tree_hash TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS undo_files (
    token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
    path TEXT NOT NULL,
    hash TEXT NOT NULL REFERENCES blobs(hash),
    size INTEGER NOT NULL,
    PRIMARY KEY (token, path)
);

CREATE TABLE IF NOT EXISTS expiry_policies (
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    target_kind INTEGER NOT NULL,
    mode INTEGER NOT NULL,
    duration_seconds INTEGER,
    deadline INTEGER,
    min_age_seconds INTEGER,
    max_age_seconds INTEGER,
    max_size_bytes INTEGER,
    power REAL,
    refreshed INTEGER,
    own_deadline INTEGER,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (site_id, path)
);

CREATE TABLE IF NOT EXISTS undo_expiry_policies (
    token TEXT NOT NULL REFERENCES undo_operations(token) ON DELETE CASCADE,
    path TEXT NOT NULL,
    target_kind INTEGER NOT NULL,
    mode INTEGER NOT NULL,
    duration_seconds INTEGER,
    deadline INTEGER,
    min_age_seconds INTEGER,
    max_age_seconds INTEGER,
    max_size_bytes INTEGER,
    power REAL,
    refreshed INTEGER,
    own_deadline INTEGER NOT NULL,
    size_bytes INTEGER NOT NULL,
    PRIMARY KEY (token, path)
);

CREATE TABLE IF NOT EXISTS idempotency_records (
    key_hash TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    operation_kind INTEGER NOT NULL,
    result_metadata TEXT NOT NULL,
    expires INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS management_tombstones (
    name TEXT PRIMARY KEY,
    management_hash BLOB NOT NULL,
    created INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS management_audit (
    id INTEGER PRIMARY KEY,
    site_name TEXT NOT NULL,
    action INTEGER NOT NULL,
    occurred INTEGER NOT NULL,
    source_ip TEXT
);

CREATE TABLE IF NOT EXISTS management_idempotency (
    key_hash TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    expires INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS path_aggregates (
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    logical_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL,
    PRIMARY KEY (site_id, path)
);

CREATE INDEX IF NOT EXISTS files_hash ON files(hash);
CREATE INDEX IF NOT EXISTS files_site_prefix ON files(site_id, path);
CREATE INDEX IF NOT EXISTS undo_operations_retention
    ON undo_operations(consumed, expires, created);
CREATE INDEX IF NOT EXISTS undo_names_stack ON undo_names(name, token);
CREATE INDEX IF NOT EXISTS undo_files_hash ON undo_files(hash);
CREATE INDEX IF NOT EXISTS expiry_policies_deadline ON expiry_policies(own_deadline);
CREATE INDEX IF NOT EXISTS expiry_policies_site_kind
    ON expiry_policies(site_id, target_kind, path);
CREATE INDEX IF NOT EXISTS idempotency_records_expiry ON idempotency_records(expires);
CREATE INDEX IF NOT EXISTS management_idempotency_expiry
    ON management_idempotency(expires);
CREATE INDEX IF NOT EXISTS management_audit_site
    ON management_audit(site_name, occurred);

PRAGMA user_version = 6;

ALTER TABLE expiry_policies ADD COLUMN size_bytes INTEGER NOT NULL DEFAULT 0;

CREATE TABLE undo_expiry_policies (
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
CREATE INDEX expiry_policies_site_kind
    ON expiry_policies(site_id, target_kind, path);

ALTER TABLE sites ADD COLUMN creator_kind INTEGER;
ALTER TABLE sites ADD COLUMN creator_hash BLOB;
ALTER TABLE sites ADD COLUMN claim_hash BLOB;
ALTER TABLE sites ADD COLUMN management_hash BLOB;
ALTER TABLE sites ADD COLUMN management_status INTEGER NOT NULL DEFAULT 0;

CREATE TABLE management_tombstones (
    name TEXT PRIMARY KEY,
    management_hash BLOB NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE management_audit (
    id INTEGER PRIMARY KEY,
    site_name TEXT NOT NULL,
    action INTEGER NOT NULL,
    occurred INTEGER NOT NULL,
    source_ip TEXT
);
CREATE TABLE management_idempotency (
    key_hash TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    expires INTEGER NOT NULL
);
CREATE INDEX management_idempotency_expiry
    ON management_idempotency(expires);
CREATE INDEX management_audit_site
    ON management_audit(site_name, occurred);

CREATE TABLE path_aggregates (
    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    logical_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL,
    PRIMARY KEY (site_id, path)
);

PRAGMA user_version = 6;

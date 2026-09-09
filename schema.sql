-- AUTO-GENERATED FROM crates/symbol/src/database/schema.rs
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS "sites" ( "id" integer PRIMARY KEY, "name" text NOT NULL UNIQUE, "created" integer, "updated" integer NOT NULL, "public_url" text NOT NULL DEFAULT '', "content_revision" integer NOT NULL DEFAULT 0, "tree_hash" blob NOT NULL DEFAULT x'0000000000000000000000000000000000000000000000000000000000000000', "creator_kind" integer, "creator_hash" blob, "claim_hash" blob, "management_hash" blob, "management_status" integer NOT NULL DEFAULT 0 );

CREATE TABLE IF NOT EXISTS "blobs" ( "hash" blob PRIMARY KEY, "bytes" blob NOT NULL DEFAULT x'', "size" integer NOT NULL );

CREATE TABLE IF NOT EXISTS "site_events" ( "id" integer PRIMARY KEY, "site_id" integer NOT NULL, "kind" integer NOT NULL, "occurred" integer NOT NULL, "files" integer NOT NULL DEFAULT 0, FOREIGN KEY ("site_id") REFERENCES "sites" ("id") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "site_entries" ( "site_id" integer NOT NULL, "path" text NOT NULL, "kind" integer NOT NULL, PRIMARY KEY ("site_id", "path"), UNIQUE ("site_id", "path", "kind"), FOREIGN KEY ("site_id") REFERENCES "sites" ("id") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "files" ( "site_id" integer NOT NULL, "path" text NOT NULL, "kind" integer NOT NULL DEFAULT 0 CHECK ("kind" = 0), "hash" blob NOT NULL, "size" integer NOT NULL, PRIMARY KEY ("site_id", "path"), FOREIGN KEY ("site_id", "path", "kind") REFERENCES "site_entries" ("site_id", "path", "kind") ON DELETE CASCADE, FOREIGN KEY ("hash") REFERENCES "blobs" ("hash") );

CREATE TABLE IF NOT EXISTS "metadata" ( "key" text PRIMARY KEY, "value" text NOT NULL );

CREATE TABLE IF NOT EXISTS "undo_operations" ( "token" text PRIMARY KEY, "kind" integer NOT NULL, "description" text NOT NULL, "created" integer NOT NULL, "expires" integer NOT NULL, "consumed" integer NOT NULL DEFAULT 0 );

CREATE TABLE IF NOT EXISTS "undo_names" ( "token" text NOT NULL, "name" text NOT NULL, PRIMARY KEY ("token", "name"), FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "undo_sites" ( "token" text PRIMARY KEY, "name" text NOT NULL, "existed" integer NOT NULL, "public_url" text NOT NULL, "created" integer, "updated" integer NOT NULL, "content_revision" integer NOT NULL, "tree_hash" blob NOT NULL, FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "undo_files" ( "token" text NOT NULL, "path" text NOT NULL, "hash" blob NOT NULL, "size" integer NOT NULL, PRIMARY KEY ("token", "path"), FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE, FOREIGN KEY ("hash") REFERENCES "blobs" ("hash") );

CREATE TABLE IF NOT EXISTS "expiry_policies" ( "site_id" integer NOT NULL, "path" text NOT NULL, "target_kind" integer NOT NULL, "mode" integer NOT NULL, "duration_seconds" integer, "deadline" integer, "min_age_seconds" integer, "max_age_seconds" integer, "max_size_bytes" integer, "power" real, "refreshed" integer, "own_deadline" integer, "size_bytes" integer NOT NULL DEFAULT 0, PRIMARY KEY ("site_id", "path"), FOREIGN KEY ("site_id") REFERENCES "sites" ("id") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "undo_expiry_policies" ( "token" text NOT NULL, "path" text NOT NULL, "target_kind" integer NOT NULL, "mode" integer NOT NULL, "duration_seconds" integer, "deadline" integer, "min_age_seconds" integer, "max_age_seconds" integer, "max_size_bytes" integer, "power" real, "refreshed" integer, "own_deadline" integer NOT NULL, "size_bytes" integer NOT NULL, PRIMARY KEY ("token", "path"), FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "idempotency_records" ( "key_hash" text PRIMARY KEY, "fingerprint" text NOT NULL, "operation_kind" integer NOT NULL, "result_metadata" text NOT NULL, "expires" integer NOT NULL );

CREATE TABLE IF NOT EXISTS "management_tombstones" ( "name" text PRIMARY KEY, "management_hash" blob NOT NULL, "created" integer NOT NULL );

CREATE TABLE IF NOT EXISTS "management_audit" ( "id" integer PRIMARY KEY, "site_name" text NOT NULL, "action" integer NOT NULL, "occurred" integer NOT NULL, "source_ip" text );

CREATE TABLE IF NOT EXISTS "management_idempotency" ( "key_hash" text PRIMARY KEY, "fingerprint" text NOT NULL, "expires" integer NOT NULL );

CREATE TABLE IF NOT EXISTS "path_aggregates" ( "site_id" integer NOT NULL, "path" text NOT NULL, "logical_bytes" integer NOT NULL, "file_count" integer NOT NULL, PRIMARY KEY ("site_id", "path"), FOREIGN KEY ("site_id") REFERENCES "sites" ("id") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "undo_file_deltas" ( "token" text NOT NULL, "path" text NOT NULL, "existed" integer NOT NULL, "kind" integer, "hash" blob, "size" integer, PRIMARY KEY ("token", "path"), FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "allocated_entries" ( "site_id" integer NOT NULL, "path" text NOT NULL, "kind" integer NOT NULL DEFAULT 1 CHECK ("kind" = 1), "hash" blob NOT NULL, "size" integer NOT NULL, "naming_mode" integer NOT NULL, "prefix" text NOT NULL, "suffix" text NOT NULL, "extension" text, "media_type" text NOT NULL, PRIMARY KEY ("site_id", "path"), FOREIGN KEY ("site_id", "path", "kind") REFERENCES "site_entries" ("site_id", "path", "kind") ON DELETE CASCADE, FOREIGN KEY ("hash") REFERENCES "blobs" ("hash") );

CREATE TABLE IF NOT EXISTS "pending_allocations" ( "token" text PRIMARY KEY, "site_id" integer NOT NULL, "folder" text NOT NULL, "hash" blob NOT NULL, "size" integer NOT NULL, "media_type" text NOT NULL, "request_fingerprint" text NOT NULL, "created" integer NOT NULL, "expires" integer NOT NULL, FOREIGN KEY ("site_id") REFERENCES "sites" ("id") ON DELETE CASCADE, FOREIGN KEY ("hash") REFERENCES "blobs" ("hash") );

CREATE TABLE IF NOT EXISTS "undo_allocated_deltas" ( "token" text NOT NULL, "path" text NOT NULL, "existed" integer NOT NULL, "hash" blob, "size" integer, "naming_mode" integer, "prefix" text, "suffix" text, "extension" text, "media_type" text, PRIMARY KEY ("token", "path"), FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "aliases" ( "site_id" integer NOT NULL, "path" text NOT NULL, "kind" integer NOT NULL DEFAULT 2 CHECK ("kind" = 2), "canonical_target" text NOT NULL, "resolved_kind" integer, "resolved_hash" blob, "resolved_size" integer, PRIMARY KEY ("site_id", "path"), FOREIGN KEY ("site_id", "path", "kind") REFERENCES "site_entries" ("site_id", "path", "kind") ON DELETE CASCADE );

CREATE TABLE IF NOT EXISTS "undo_alias_deltas" ( "token" text NOT NULL, "path" text NOT NULL, "existed" integer NOT NULL, "canonical_target" text, "resolved_kind" integer, "resolved_hash" blob, "resolved_size" integer, PRIMARY KEY ("token", "path"), FOREIGN KEY ("token") REFERENCES "undo_operations" ("token") ON DELETE CASCADE );

CREATE INDEX IF NOT EXISTS "files_hash" ON "files" ("hash");
CREATE INDEX IF NOT EXISTS "files_site_prefix" ON "files" ("site_id", "path");
CREATE INDEX IF NOT EXISTS "files_site_hash" ON "files" ("site_id", "hash");
CREATE INDEX IF NOT EXISTS "undo_operations_retention" ON "undo_operations" ("consumed", "expires", "created");
CREATE INDEX IF NOT EXISTS "undo_names_stack" ON "undo_names" ("name", "token");
CREATE INDEX IF NOT EXISTS "undo_files_hash" ON "undo_files" ("hash");
CREATE INDEX IF NOT EXISTS "expiry_policies_deadline" ON "expiry_policies" ("own_deadline");
CREATE INDEX IF NOT EXISTS "expiry_policies_site_kind" ON "expiry_policies" ("site_id", "target_kind", "path");
CREATE INDEX IF NOT EXISTS "idempotency_records_expiry" ON "idempotency_records" ("expires");
CREATE INDEX IF NOT EXISTS "management_idempotency_expiry" ON "management_idempotency" ("expires");
CREATE INDEX IF NOT EXISTS "management_audit_site" ON "management_audit" ("site_name", "occurred");
CREATE INDEX IF NOT EXISTS "pending_allocations_expiry" ON "pending_allocations" ("expires");
CREATE INDEX IF NOT EXISTS "aliases_dependency" ON "aliases" ("site_id", "canonical_target");
CREATE INDEX IF NOT EXISTS "site_events_site" ON "site_events" ("site_id", "occurred");
CREATE INDEX IF NOT EXISTS "aliases_cache" ON "aliases" ("site_id", "resolved_kind", "resolved_hash", "resolved_size");

PRAGMA user_version = 11;

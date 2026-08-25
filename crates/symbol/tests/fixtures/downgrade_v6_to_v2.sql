DROP TABLE path_aggregates;
DROP INDEX management_idempotency_expiry;
DROP INDEX management_audit_site;
DROP TABLE management_idempotency;
DROP TABLE management_audit;
DROP TABLE management_tombstones;

ALTER TABLE sites DROP COLUMN creator_kind;
ALTER TABLE sites DROP COLUMN creator_hash;
ALTER TABLE sites DROP COLUMN claim_hash;
ALTER TABLE sites DROP COLUMN management_hash;
ALTER TABLE sites DROP COLUMN management_status;

DROP INDEX expiry_policies_site_kind;
DROP TABLE undo_expiry_policies;
ALTER TABLE expiry_policies DROP COLUMN size_bytes;

PRAGMA user_version = 2;

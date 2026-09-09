use std::fmt::Write as _;

use sea_query::{
    Alias, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, IntoIden, IntoTableRef, SqliteQueryBuilder, Table, TableCreateStatement,
};
#[cfg(test)]
use sea_query::Query;

pub const LATEST_SCHEMA_VERSION: i64 = 11;
pub const FILE_ENTRY_KIND: i64 = 0;
pub const ALLOCATED_ENTRY_KIND: i64 = 1;
pub const ALIAS_ENTRY_KIND: i64 = 2;

#[derive(Iden)]
enum Sites {
    Table,
    Id,
    Name,
    Created,
    Updated,
    PublicUrl,
    ContentRevision,
    TreeHash,
    CreatorKind,
    CreatorHash,
    ClaimHash,
    ManagementHash,
    ManagementStatus,
}

#[derive(Iden)]
enum Blobs {
    Table,
    Hash,
    Bytes,
    Size,
}

#[derive(Iden)]
enum Files {
    Table,
    SiteId,
    Path,
    Kind,
    Hash,
    Size,
}

#[derive(Iden)]
enum FilesV6 {
    Table,
}

#[derive(Iden)]
enum FilesV8 {
    Table,
}

#[cfg(test)]
#[derive(Iden)]
enum FilesV9 {
    Table,
}

#[derive(Iden)]
enum BlobsV10 {
    Table,
}

#[derive(Iden)]
enum FilesV10 {
    Table,
}

#[derive(Iden)]
enum UndoFilesV10 {
    Table,
}

#[derive(Iden)]
enum AllocatedEntriesV10 {
    Table,
}

#[derive(Iden)]
enum PendingAllocationsV10 {
    Table,
}

#[derive(Iden)]
enum UndoFileDeltasV10 {
    Table,
}

#[derive(Iden)]
enum UndoAllocatedDeltasV10 {
    Table,
}

#[derive(Iden)]
enum AliasesV10 {
    Table,
}

#[derive(Iden)]
enum UndoAliasDeltasV10 {
    Table,
}

#[derive(Iden)]
enum SitesV10 {
    Table,
}

#[derive(Iden)]
enum UndoSitesV10 {
    Table,
}

#[derive(Iden)]
enum SitesRepair {
    Table,
}

#[derive(Iden)]
enum ExpiryPoliciesRepair {
    Table,
}

#[derive(Iden)]
enum PathAggregatesRepair {
    Table,
}

#[derive(Iden)]
enum BlobsRepair {
    Table,
}

#[derive(Iden)]
enum FilesRepair {
    Table,
}

#[derive(Iden)]
enum FilesRepairSource {
    Table,
}

#[derive(Iden)]
enum UndoFilesRepair {
    Table,
}

#[derive(Iden)]
enum UndoFilesRepairSource {
    Table,
}

#[derive(Iden)]
enum UndoSitesRepair {
    Table,
}

#[derive(Iden)]
enum SiteEntries {
    Table,
    SiteId,
    Path,
    Kind,
}

#[derive(Iden)]
enum UndoFileDeltas {
    Table,
    Token,
    Path,
    Existed,
    Kind,
    Hash,
    Size,
}

#[derive(Iden)]
enum AllocatedEntries {
    Table,
    SiteId,
    Path,
    Kind,
    Hash,
    Size,
    NamingMode,
    Prefix,
    Suffix,
    Extension,
    MediaType,
}

#[derive(Iden)]
enum AllocatedEntriesV8 {
    Table,
}

#[derive(Iden)]
enum PendingAllocations {
    Table,
    Token,
    SiteId,
    Folder,
    Hash,
    Size,
    MediaType,
    RequestFingerprint,
    Created,
    Expires,
}

#[derive(Iden)]
enum UndoAllocatedDeltas {
    Table,
    Token,
    Path,
    Existed,
    Hash,
    Size,
    NamingMode,
    Prefix,
    Suffix,
    Extension,
    MediaType,
}

#[derive(Iden)]
enum Aliases {
    Table,
    SiteId,
    Path,
    Kind,
    CanonicalTarget,
    ResolvedKind,
    ResolvedHash,
    ResolvedSize,
}

#[derive(Iden)]
enum UndoAliasDeltas {
    Table,
    Token,
    Path,
    Existed,
    CanonicalTarget,
    ResolvedKind,
    ResolvedHash,
    ResolvedSize,
}

#[derive(Iden)]
enum SiteEvents {
    Table,
    Id,
    SiteId,
    Kind,
    Occurred,
    Files,
}

#[derive(Iden)]
enum Metadata {
    Table,
    Key,
    Value,
}

#[derive(Iden)]
enum UndoOperations {
    Table,
    Token,
    Kind,
    Description,
    Created,
    Expires,
    Consumed,
}

#[derive(Iden)]
enum UndoNames {
    Table,
    Token,
    Name,
}

#[derive(Iden)]
enum UndoSites {
    Table,
    Token,
    Name,
    Existed,
    PublicUrl,
    Created,
    Updated,
    ContentRevision,
    TreeHash,
}

#[derive(Iden)]
enum UndoFiles {
    Table,
    Token,
    Path,
    Hash,
    Size,
}

#[derive(Iden)]
enum ExpiryPolicies {
    Table,
    SiteId,
    Path,
    TargetKind,
    Mode,
    DurationSeconds,
    Deadline,
    MinAgeSeconds,
    MaxAgeSeconds,
    MaxSizeBytes,
    Power,
    Refreshed,
    OwnDeadline,
    SizeBytes,
}

#[derive(Iden)]
enum UndoExpiryPolicies {
    Table,
    Token,
    Path,
    TargetKind,
    Mode,
    DurationSeconds,
    Deadline,
    MinAgeSeconds,
    MaxAgeSeconds,
    MaxSizeBytes,
    Power,
    Refreshed,
    OwnDeadline,
    SizeBytes,
}

#[derive(Iden)]
enum IdempotencyRecords {
    Table,
    KeyHash,
    Fingerprint,
    OperationKind,
    ResultMetadata,
    Expires,
}

#[derive(Iden)]
enum ManagementTombstones {
    Table,
    Name,
    ManagementHash,
    Created,
}

#[derive(Iden)]
enum ManagementAudit {
    Table,
    Id,
    SiteName,
    Action,
    Occurred,
    SourceIp,
}

#[derive(Iden)]
enum ManagementIdempotency {
    Table,
    KeyHash,
    Fingerprint,
    Expires,
}

#[derive(Iden)]
enum PathAggregates {
    Table,
    SiteId,
    Path,
    LogicalBytes,
    FileCount,
}

pub fn tables() -> Vec<TableCreateStatement> {
    vec![
        sites_table(),
        blobs_table(),
        site_events_table(),
        site_entries_table(),
        files_table(),
        metadata_table(),
        undo_operations_table(),
        undo_names_table(),
        undo_sites_table(),
        undo_files_table(),
        expiry_policies_table(),
        undo_expiry_policies_table(),
        idempotency_records_table(),
        management_tombstones_table(),
        management_audit_table(),
        management_idempotency_table(),
        path_aggregates_table(),
        undo_file_deltas_table(),
        allocated_entries_table(),
        pending_allocations_table(),
        undo_allocated_deltas_table(),
        aliases_table(),
        undo_alias_deltas_table(),
    ]
}

pub fn indexes() -> Vec<IndexCreateStatement> {
    vec![
        index("files_hash", Files::Table, [Files::Hash]),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        ),
        index(
            "files_site_hash",
            Files::Table,
            [Files::SiteId, Files::Hash],
        ),
        index(
            "undo_operations_retention",
            UndoOperations::Table,
            [
                UndoOperations::Consumed,
                UndoOperations::Expires,
                UndoOperations::Created,
            ],
        ),
        index(
            "undo_names_stack",
            UndoNames::Table,
            [UndoNames::Name, UndoNames::Token],
        ),
        index("undo_files_hash", UndoFiles::Table, [UndoFiles::Hash]),
        index(
            "expiry_policies_deadline",
            ExpiryPolicies::Table,
            [ExpiryPolicies::OwnDeadline],
        ),
        index(
            "expiry_policies_site_kind",
            ExpiryPolicies::Table,
            [
                ExpiryPolicies::SiteId,
                ExpiryPolicies::TargetKind,
                ExpiryPolicies::Path,
            ],
        ),
        index(
            "idempotency_records_expiry",
            IdempotencyRecords::Table,
            [IdempotencyRecords::Expires],
        ),
        index(
            "management_idempotency_expiry",
            ManagementIdempotency::Table,
            [ManagementIdempotency::Expires],
        ),
        index(
            "management_audit_site",
            ManagementAudit::Table,
            [ManagementAudit::SiteName, ManagementAudit::Occurred],
        ),
        index(
            "pending_allocations_expiry",
            PendingAllocations::Table,
            [PendingAllocations::Expires],
        ),
        index(
            "aliases_dependency",
            Aliases::Table,
            [Aliases::SiteId, Aliases::CanonicalTarget],
        ),
        index(
            "site_events_site",
            SiteEvents::Table,
            [SiteEvents::SiteId, SiteEvents::Occurred],
        ),
        index(
            "aliases_cache",
            Aliases::Table,
            [
                Aliases::SiteId,
                Aliases::ResolvedKind,
                Aliases::ResolvedHash,
                Aliases::ResolvedSize,
            ],
        ),
    ]
}

pub fn schema_sql() -> String {
    let mut sql = String::from("-- AUTO-GENERATED FROM crates/symbol/src/database/schema.rs\n");
    sql.push_str("PRAGMA foreign_keys = ON;\n\n");
    for table in tables() {
        sql.push_str(&table.to_string(SqliteQueryBuilder));
        sql.push_str(";\n\n");
    }
    for index in indexes() {
        sql.push_str(&index.to_string(SqliteQueryBuilder));
        sql.push_str(";\n");
    }
    writeln!(sql, "\nPRAGMA user_version = {LATEST_SCHEMA_VERSION};")
        .expect("writing to String cannot fail");
    sql
}

pub fn schema_v6_sql() -> String {
    let tables = vec![
        sites_v6_table(),
        blobs_table_v10(),
        files_v6_table(),
        metadata_table(),
        undo_operations_table(),
        undo_names_table(),
        undo_sites_v6_table(),
        undo_files_table_v10(),
        expiry_policies_table(),
        undo_expiry_policies_table(),
        idempotency_records_table(),
        management_tombstones_table(),
        management_audit_table(),
        management_idempotency_table(),
        path_aggregates_table(),
    ];
    let indexes = vec![
        index("files_hash", Files::Table, [Files::Hash]),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        ),
        index(
            "undo_operations_retention",
            UndoOperations::Table,
            [
                UndoOperations::Consumed,
                UndoOperations::Expires,
                UndoOperations::Created,
            ],
        ),
        index(
            "undo_names_stack",
            UndoNames::Table,
            [UndoNames::Name, UndoNames::Token],
        ),
        index("undo_files_hash", UndoFiles::Table, [UndoFiles::Hash]),
        index(
            "expiry_policies_deadline",
            ExpiryPolicies::Table,
            [ExpiryPolicies::OwnDeadline],
        ),
        index(
            "expiry_policies_site_kind",
            ExpiryPolicies::Table,
            [
                ExpiryPolicies::SiteId,
                ExpiryPolicies::TargetKind,
                ExpiryPolicies::Path,
            ],
        ),
        index(
            "idempotency_records_expiry",
            IdempotencyRecords::Table,
            [IdempotencyRecords::Expires],
        ),
        index(
            "management_idempotency_expiry",
            ManagementIdempotency::Table,
            [ManagementIdempotency::Expires],
        ),
        index(
            "management_audit_site",
            ManagementAudit::Table,
            [ManagementAudit::SiteName, ManagementAudit::Occurred],
        ),
    ];
    let mut sql = String::from("-- AUTO-GENERATED FROM crates/symbol/src/database/schema.rs\n");
    sql.push_str("PRAGMA foreign_keys = ON;\n\n");
    for table in tables {
        sql.push_str(&table.to_string(SqliteQueryBuilder));
        sql.push_str(";\n\n");
    }
    for index in indexes {
        sql.push_str(&index.to_string(SqliteQueryBuilder));
        sql.push_str(";\n");
    }
    sql.push_str("\nPRAGMA user_version = 6;\n");
    sql
}

pub fn upgrade_v2_to_v6() -> Vec<String> {
    vec![
        Table::alter()
            .table(ExpiryPolicies::Table)
            .add_column(
                ColumnDef::new(ExpiryPolicies::SizeBytes)
                    .integer()
                    .not_null()
                    .default(0),
            )
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_expiry_policies_table().to_string(SqliteQueryBuilder),
        Index::create()
            .name("expiry_policies_site_kind")
            .table(ExpiryPolicies::Table)
            .col(ExpiryPolicies::SiteId)
            .col(ExpiryPolicies::TargetKind)
            .col(ExpiryPolicies::Path)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .add_column(ColumnDef::new(Sites::CreatorKind).integer())
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .add_column(ColumnDef::new(Sites::CreatorHash).blob())
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .add_column(ColumnDef::new(Sites::ClaimHash).blob())
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .add_column(ColumnDef::new(Sites::ManagementHash).blob())
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .add_column(
                ColumnDef::new(Sites::ManagementStatus)
                    .integer()
                    .not_null()
                    .default(0),
            )
            .to_owned()
            .to_string(SqliteQueryBuilder),
        management_tombstones_table().to_string(SqliteQueryBuilder),
        management_audit_table().to_string(SqliteQueryBuilder),
        management_idempotency_table().to_string(SqliteQueryBuilder),
        Index::create()
            .name("management_idempotency_expiry")
            .table(ManagementIdempotency::Table)
            .col(ManagementIdempotency::Expires)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Index::create()
            .name("management_audit_site")
            .table(ManagementAudit::Table)
            .col(ManagementAudit::SiteName)
            .col(ManagementAudit::Occurred)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        path_aggregates_table().to_string(SqliteQueryBuilder),
    ]
}

pub fn upgrade_v6_to_v7_before_copy() -> Vec<String> {
    vec![site_entries_table().to_string(SqliteQueryBuilder)]
}

pub fn upgrade_v6_to_v7_after_backfill() -> Vec<String> {
    vec![
        Table::rename()
            .table(Files::Table, FilesV6::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        files_table_v10().to_string(SqliteQueryBuilder),
    ]
}

pub fn upgrade_v6_to_v7_after_file_copy() -> Vec<String> {
    vec![
        Table::drop()
            .table(FilesV6::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("files_hash", Files::Table, [Files::Hash]).to_string(SqliteQueryBuilder),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        )
        .to_string(SqliteQueryBuilder),
        undo_file_deltas_table_v10().to_string(SqliteQueryBuilder),
    ]
}

pub fn upgrade_v7_to_v8() -> Vec<String> {
    vec![
        allocated_entries_table_v10().to_string(SqliteQueryBuilder),
        pending_allocations_table_v10().to_string(SqliteQueryBuilder),
        undo_allocated_deltas_table_v10().to_string(SqliteQueryBuilder),
        index(
            "files_site_hash",
            Files::Table,
            [Files::SiteId, Files::Hash],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "pending_allocations_expiry",
            PendingAllocations::Table,
            [PendingAllocations::Expires],
        )
        .to_string(SqliteQueryBuilder),
    ]
}

pub fn upgrade_v9_to_v10() -> Vec<String> {
    vec![
        Table::alter()
            .table(Sites::Table)
            .add_column(ColumnDef::new(Sites::Created).integer())
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(UndoSites::Table)
            .add_column(ColumnDef::new(UndoSites::Created).integer())
            .to_owned()
            .to_string(SqliteQueryBuilder),
        site_events_table().to_string(SqliteQueryBuilder),
        index(
            "site_events_site",
            SiteEvents::Table,
            [SiteEvents::SiteId, SiteEvents::Occurred],
        )
        .to_string(SqliteQueryBuilder),
    ]
}

#[expect(clippy::too_many_lines)]
pub fn upgrade_v10_to_v11() -> Vec<String> {
    vec![
        "PRAGMA foreign_keys=OFF".to_string(),
        Table::rename()
            .table(Blobs::Table, BlobsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        blobs_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"blobs\" (\"hash\", \"bytes\", \"size\")
         SELECT unhex(\"hash\"), \"bytes\", \"size\" FROM \"blobs_v10\"".to_string(),
        Table::rename()
            .table(Files::Table, FilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        files_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"files\" (\"site_id\", \"path\", \"kind\", \"hash\", \"size\")
         SELECT \"site_id\", \"path\", \"kind\", unhex(\"hash\"), \"size\" FROM \"files_v10\""
            .to_string(),
        Table::drop()
            .table(FilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("files_hash", Files::Table, [Files::Hash]).to_string(SqliteQueryBuilder),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "files_site_hash",
            Files::Table,
            [Files::SiteId, Files::Hash],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoFiles::Table, UndoFilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_files_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_files\" (\"token\", \"path\", \"hash\", \"size\")
         SELECT \"token\", \"path\", unhex(\"hash\"), \"size\" FROM \"undo_files_v10\""
            .to_string(),
        Table::drop()
            .table(UndoFilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("undo_files_hash", UndoFiles::Table, [UndoFiles::Hash])
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(AllocatedEntries::Table, AllocatedEntriesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        allocated_entries_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"allocated_entries\"
            (\"site_id\", \"path\", \"kind\", \"hash\", \"size\", \"naming_mode\", \"prefix\",
             \"suffix\", \"extension\", \"media_type\")
         SELECT \"site_id\", \"path\", \"kind\", unhex(\"hash\"), \"size\", \"naming_mode\",
                \"prefix\", \"suffix\", \"extension\", \"media_type\"
         FROM \"allocated_entries_v10\""
            .to_string(),
        Table::drop()
            .table(AllocatedEntriesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(PendingAllocations::Table, PendingAllocationsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        pending_allocations_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"pending_allocations\"
            (\"token\", \"site_id\", \"folder\", \"hash\", \"size\", \"media_type\",
             \"request_fingerprint\", \"created\", \"expires\")
         SELECT \"token\", \"site_id\", \"folder\", unhex(\"hash\"), \"size\", \"media_type\",
                \"request_fingerprint\", \"created\", \"expires\"
         FROM \"pending_allocations_v10\""
            .to_string(),
        Table::drop()
            .table(PendingAllocationsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index(
            "pending_allocations_expiry",
            PendingAllocations::Table,
            [PendingAllocations::Expires],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoFileDeltas::Table, UndoFileDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_file_deltas_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_file_deltas\"
            (\"token\", \"path\", \"existed\", \"kind\", \"hash\", \"size\")
         SELECT \"token\", \"path\", \"existed\", \"kind\",
                CASE WHEN \"hash\" IS NULL THEN NULL ELSE unhex(\"hash\") END,
                \"size\"
         FROM \"undo_file_deltas_v10\""
            .to_string(),
        Table::drop()
            .table(UndoFileDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoAllocatedDeltas::Table, UndoAllocatedDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_allocated_deltas_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_allocated_deltas\"
            (\"token\", \"path\", \"existed\", \"hash\", \"size\", \"naming_mode\", \"prefix\",
             \"suffix\", \"extension\", \"media_type\")
         SELECT \"token\", \"path\", \"existed\",
                CASE WHEN \"hash\" IS NULL THEN NULL ELSE unhex(\"hash\") END,
                \"size\", \"naming_mode\", \"prefix\", \"suffix\", \"extension\", \"media_type\"
         FROM \"undo_allocated_deltas_v10\""
            .to_string(),
        Table::drop()
            .table(UndoAllocatedDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(Aliases::Table, AliasesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        aliases_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"aliases\"
            (\"site_id\", \"path\", \"kind\", \"canonical_target\", \"resolved_kind\",
             \"resolved_hash\", \"resolved_size\")
         SELECT \"site_id\", \"path\", \"kind\", \"canonical_target\", \"resolved_kind\",
                CASE WHEN \"resolved_hash\" IS NULL THEN NULL ELSE unhex(\"resolved_hash\") END,
                \"resolved_size\"
         FROM \"aliases_v10\""
            .to_string(),
        Table::drop()
            .table(AliasesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index(
            "aliases_dependency",
            Aliases::Table,
            [Aliases::SiteId, Aliases::CanonicalTarget],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "aliases_cache",
            Aliases::Table,
            [
                Aliases::SiteId,
                Aliases::ResolvedKind,
                Aliases::ResolvedHash,
                Aliases::ResolvedSize,
            ],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoAliasDeltas::Table, UndoAliasDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_alias_deltas_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_alias_deltas\"
            (\"token\", \"path\", \"existed\", \"canonical_target\", \"resolved_kind\",
             \"resolved_hash\", \"resolved_size\")
         SELECT \"token\", \"path\", \"existed\", \"canonical_target\", \"resolved_kind\",
                CASE WHEN \"resolved_hash\" IS NULL THEN NULL ELSE unhex(\"resolved_hash\") END,
                \"resolved_size\"
         FROM \"undo_alias_deltas_v10\""
            .to_string(),
        Table::drop()
            .table(UndoAliasDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        "ALTER TABLE \"sites\" ADD COLUMN \"tree_hash_bin\" BLOB".to_string(),
        "UPDATE \"sites\" SET \"tree_hash_bin\" = CASE
            WHEN \"tree_hash\" = '' THEN zeroblob(32)
            WHEN \"tree_hash\" LIKE 'blake3:%' THEN unhex(substr(\"tree_hash\", 8))
            ELSE unhex(\"tree_hash\")
        END"
            .to_string(),
        "ALTER TABLE \"sites\" DROP COLUMN \"tree_hash\"".to_string(),
        "ALTER TABLE \"sites\" RENAME COLUMN \"tree_hash_bin\" TO \"tree_hash\"".to_string(),
        "ALTER TABLE \"undo_sites\" ADD COLUMN \"tree_hash_bin\" BLOB".to_string(),
        "UPDATE \"undo_sites\" SET \"tree_hash_bin\" = CASE
            WHEN \"tree_hash\" = '' THEN zeroblob(32)
            WHEN \"tree_hash\" LIKE 'blake3:%' THEN unhex(substr(\"tree_hash\", 8))
            ELSE unhex(\"tree_hash\")
        END"
            .to_string(),
        "ALTER TABLE \"undo_sites\" DROP COLUMN \"tree_hash\"".to_string(),
        "ALTER TABLE \"undo_sites\" RENAME COLUMN \"tree_hash_bin\" TO \"tree_hash\"".to_string(),
        Table::drop()
            .table(BlobsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        "PRAGMA foreign_keys=ON".to_string(),
    ]
}

#[cfg(test)]
pub fn downgrade_v11_to_v10() -> Vec<String> {
    vec![
        "ALTER TABLE \"sites\" ADD COLUMN \"tree_hash_text\" TEXT".to_string(),
        "UPDATE \"sites\" SET \"tree_hash_text\" = CASE
            WHEN \"tree_hash\" IS NULL OR \"tree_hash\" = zeroblob(32) THEN ''
            ELSE lower(hex(\"tree_hash\"))
        END"
            .to_string(),
        "ALTER TABLE \"sites\" DROP COLUMN \"tree_hash\"".to_string(),
        "ALTER TABLE \"sites\" RENAME COLUMN \"tree_hash_text\" TO \"tree_hash\"".to_string(),
        "ALTER TABLE \"undo_sites\" ADD COLUMN \"tree_hash_text\" TEXT".to_string(),
        "UPDATE \"undo_sites\" SET \"tree_hash_text\" = CASE
            WHEN \"tree_hash\" IS NULL OR \"tree_hash\" = zeroblob(32) THEN ''
            ELSE lower(hex(\"tree_hash\"))
        END"
            .to_string(),
        "ALTER TABLE \"undo_sites\" DROP COLUMN \"tree_hash\"".to_string(),
        "ALTER TABLE \"undo_sites\" RENAME COLUMN \"tree_hash_text\" TO \"tree_hash\"".to_string(),
        Table::rename()
            .table(Blobs::Table, BlobsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        blobs_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"blobs\" (\"hash\", \"bytes\", \"size\")
         SELECT lower(hex(\"hash\")), \"bytes\", \"size\" FROM \"blobs_v10\""
            .to_string(),
        Table::drop()
            .table(BlobsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(Files::Table, FilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        files_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"files\" (\"site_id\", \"path\", \"kind\", \"hash\", \"size\")
         SELECT \"site_id\", \"path\", \"kind\", lower(hex(\"hash\")), \"size\" FROM \"files_v10\""
            .to_string(),
        Table::drop()
            .table(FilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("files_hash", Files::Table, [Files::Hash]).to_string(SqliteQueryBuilder),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "files_site_hash",
            Files::Table,
            [Files::SiteId, Files::Hash],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoFiles::Table, UndoFilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_files_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_files\" (\"token\", \"path\", \"hash\", \"size\")
         SELECT \"token\", \"path\", lower(hex(\"hash\")), \"size\" FROM \"undo_files_v10\""
            .to_string(),
        Table::drop()
            .table(UndoFilesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("undo_files_hash", UndoFiles::Table, [UndoFiles::Hash])
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(AllocatedEntries::Table, AllocatedEntriesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        allocated_entries_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"allocated_entries\"
            (\"site_id\", \"path\", \"kind\", \"hash\", \"size\", \"naming_mode\", \"prefix\",
             \"suffix\", \"extension\", \"media_type\")
         SELECT \"site_id\", \"path\", \"kind\", lower(hex(\"hash\")), \"size\", \"naming_mode\",
                \"prefix\", \"suffix\", \"extension\", \"media_type\"
         FROM \"allocated_entries_v10\""
            .to_string(),
        Table::drop()
            .table(AllocatedEntriesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(PendingAllocations::Table, PendingAllocationsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        pending_allocations_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"pending_allocations\"
            (\"token\", \"site_id\", \"folder\", \"hash\", \"size\", \"media_type\",
             \"request_fingerprint\", \"created\", \"expires\")
         SELECT \"token\", \"site_id\", \"folder\", lower(hex(\"hash\")), \"size\", \"media_type\",
                \"request_fingerprint\", \"created\", \"expires\"
         FROM \"pending_allocations_v10\""
            .to_string(),
        Table::drop()
            .table(PendingAllocationsV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index(
            "pending_allocations_expiry",
            PendingAllocations::Table,
            [PendingAllocations::Expires],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoFileDeltas::Table, UndoFileDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_file_deltas_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_file_deltas\"
            (\"token\", \"path\", \"existed\", \"kind\", \"hash\", \"size\")
         SELECT \"token\", \"path\", \"existed\", \"kind\",
                CASE WHEN \"hash\" IS NULL THEN NULL ELSE lower(hex(\"hash\")) END,
                \"size\"
         FROM \"undo_file_deltas_v10\""
            .to_string(),
        Table::drop()
            .table(UndoFileDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoAllocatedDeltas::Table, UndoAllocatedDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_allocated_deltas_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_allocated_deltas\"
            (\"token\", \"path\", \"existed\", \"hash\", \"size\", \"naming_mode\", \"prefix\",
             \"suffix\", \"extension\", \"media_type\")
         SELECT \"token\", \"path\", \"existed\",
                CASE WHEN \"hash\" IS NULL THEN NULL ELSE lower(hex(\"hash\")) END,
                \"size\", \"naming_mode\", \"prefix\", \"suffix\", \"extension\", \"media_type\"
         FROM \"undo_allocated_deltas_v10\""
            .to_string(),
        Table::drop()
            .table(UndoAllocatedDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(Aliases::Table, AliasesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        aliases_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"aliases\"
            (\"site_id\", \"path\", \"kind\", \"canonical_target\", \"resolved_kind\",
             \"resolved_hash\", \"resolved_size\")
         SELECT \"site_id\", \"path\", \"kind\", \"canonical_target\", \"resolved_kind\",
                CASE WHEN \"resolved_hash\" IS NULL THEN NULL ELSE lower(hex(\"resolved_hash\")) END,
                \"resolved_size\"
         FROM \"aliases_v10\""
            .to_string(),
        Table::drop()
            .table(AliasesV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index(
            "aliases_dependency",
            Aliases::Table,
            [Aliases::SiteId, Aliases::CanonicalTarget],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "aliases_cache",
            Aliases::Table,
            [
                Aliases::SiteId,
                Aliases::ResolvedKind,
                Aliases::ResolvedHash,
                Aliases::ResolvedSize,
            ],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoAliasDeltas::Table, UndoAliasDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_alias_deltas_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_alias_deltas\"
            (\"token\", \"path\", \"existed\", \"canonical_target\", \"resolved_kind\",
             \"resolved_hash\", \"resolved_size\")
         SELECT \"token\", \"path\", \"existed\", \"canonical_target\", \"resolved_kind\",
                CASE WHEN \"resolved_hash\" IS NULL THEN NULL ELSE lower(hex(\"resolved_hash\")) END,
                \"resolved_size\"
         FROM \"undo_alias_deltas_v10\""
            .to_string(),
        Table::drop()
            .table(UndoAliasDeltasV10::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
    ]
}

pub fn upgrade_v8_to_v9() -> Vec<String> {
    vec![
        Table::rename()
            .table(Files::Table, FilesV8::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(AllocatedEntries::Table, AllocatedEntriesV8::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        files_table_v10().to_string(SqliteQueryBuilder),
        allocated_entries_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"files\" (\"site_id\", \"path\", \"kind\", \"hash\", \"size\")
         SELECT \"site_id\", \"path\", \"kind\", \"hash\", \"size\" FROM \"files_v8\""
            .to_string(),
        "INSERT INTO \"allocated_entries\"
            (\"site_id\", \"path\", \"kind\", \"hash\", \"size\", \"naming_mode\", \"prefix\",
             \"suffix\", \"extension\", \"media_type\")
         SELECT \"site_id\", \"path\", \"kind\", \"hash\", \"size\", \"naming_mode\", \"prefix\",
                \"suffix\", \"extension\", \"media_type\"
         FROM \"allocated_entries_v8\""
            .to_string(),
        Table::drop()
            .table(FilesV8::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(AllocatedEntriesV8::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("files_hash", Files::Table, [Files::Hash]).to_string(SqliteQueryBuilder),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "files_site_hash",
            Files::Table,
            [Files::SiteId, Files::Hash],
        )
        .to_string(SqliteQueryBuilder),
        aliases_table_v10().to_string(SqliteQueryBuilder),
        undo_alias_deltas_table_v10().to_string(SqliteQueryBuilder),
        index(
            "aliases_dependency",
            Aliases::Table,
            [Aliases::SiteId, Aliases::CanonicalTarget],
        )
        .to_string(SqliteQueryBuilder),
        index(
            "aliases_cache",
            Aliases::Table,
            [
                Aliases::SiteId,
                Aliases::ResolvedKind,
                Aliases::ResolvedHash,
                Aliases::ResolvedSize,
            ],
        )
        .to_string(SqliteQueryBuilder),
    ]
}

#[cfg(test)]
pub fn downgrade_v9_to_v6_before_copy() -> Vec<String> {
    vec![
        Table::drop()
            .table(SiteEvents::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        "ALTER TABLE \"sites\" DROP COLUMN \"created\"".to_string(),
        "ALTER TABLE \"undo_sites\" DROP COLUMN \"created\"".to_string(),
        Table::drop()
            .table(UndoAliasDeltas::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(Aliases::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(UndoAllocatedDeltas::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(PendingAllocations::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(AllocatedEntries::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(UndoFileDeltas::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(Files::Table, FilesV9::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        files_v6_table().to_string(SqliteQueryBuilder),
    ]
}

#[cfg(test)]
pub fn downgrade_v9_to_v6_after_copy() -> Vec<String> {
    vec![
        Table::drop()
            .table(FilesV9::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(SiteEntries::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("files_hash", Files::Table, [Files::Hash]).to_string(SqliteQueryBuilder),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        )
        .to_string(SqliteQueryBuilder),
    ]
}

#[cfg(test)]
pub fn insert_v6_file(site_id: i64, path: &str, hash: &str, size: i64) -> String {
    Query::insert()
        .into_table(Files::Table)
        .columns([Files::SiteId, Files::Path, Files::Hash, Files::Size])
        .values_panic([site_id.into(), path.into(), hash.into(), size.into()])
        .to_owned()
        .to_string(SqliteQueryBuilder)
}

#[cfg(test)]
pub fn downgrade_v6_to_v2() -> Vec<String> {
    vec![
        Table::drop()
            .table(PathAggregates::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Index::drop()
            .name("management_idempotency_expiry")
            .table(ManagementIdempotency::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Index::drop()
            .name("management_audit_site")
            .table(ManagementAudit::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(ManagementIdempotency::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(ManagementAudit::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(ManagementTombstones::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .drop_column(Sites::CreatorKind)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .drop_column(Sites::CreatorHash)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .drop_column(Sites::ClaimHash)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .drop_column(Sites::ManagementHash)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(Sites::Table)
            .drop_column(Sites::ManagementStatus)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Index::drop()
            .name("expiry_policies_site_kind")
            .table(ExpiryPolicies::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(UndoExpiryPolicies::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::alter()
            .table(ExpiryPolicies::Table)
            .drop_column(ExpiryPolicies::SizeBytes)
            .to_owned()
            .to_string(SqliteQueryBuilder),
    ]
}

#[cfg(test)]
pub fn repair_downgraded_v6_schema() -> Vec<String> {
    vec![
        "CREATE TABLE \"files_repair_source\" AS
         SELECT \"site_id\", \"path\", \"hash\", \"size\" FROM \"files\""
            .to_string(),
        "CREATE TABLE \"undo_files_repair_source\" AS
         SELECT \"token\", \"path\", \"hash\", \"size\" FROM \"undo_files\""
            .to_string(),
        Table::rename()
            .table(Sites::Table, SitesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        sites_v6_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"sites\"
            (\"id\", \"name\", \"updated\", \"public_url\", \"content_revision\", \"tree_hash\",
             \"creator_kind\", \"creator_hash\", \"claim_hash\", \"management_hash\",
             \"management_status\")
         SELECT \"id\", \"name\", \"updated\", \"public_url\", \"content_revision\",
                COALESCE(\"tree_hash\", ''),
                \"creator_kind\", \"creator_hash\", \"claim_hash\", \"management_hash\",
                \"management_status\"
         FROM \"sites_repair\""
            .to_string(),
        Table::drop()
            .table(SitesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(ExpiryPolicies::Table, ExpiryPoliciesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        expiry_policies_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"expiry_policies\"
            (\"site_id\", \"path\", \"target_kind\", \"mode\", \"duration_seconds\", \"deadline\",
             \"min_age_seconds\", \"max_age_seconds\", \"max_size_bytes\", \"power\", \"refreshed\",
             \"own_deadline\")
         SELECT \"site_id\", \"path\", \"target_kind\", \"mode\", \"duration_seconds\", \"deadline\",
                \"min_age_seconds\", \"max_age_seconds\", \"max_size_bytes\", \"power\", \"refreshed\",
                \"own_deadline\"
         FROM \"expiry_policies_repair\""
            .to_string(),
        Table::drop()
            .table(ExpiryPoliciesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Index::create()
            .name("expiry_policies_site_kind")
            .table(ExpiryPolicies::Table)
            .col(ExpiryPolicies::SiteId)
            .col(ExpiryPolicies::TargetKind)
            .col(ExpiryPolicies::Path)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Index::create()
            .name("expiry_policies_deadline")
            .table(ExpiryPolicies::Table)
            .col(ExpiryPolicies::OwnDeadline)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(Blobs::Table, BlobsRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        blobs_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"blobs\" (\"hash\", \"bytes\", \"size\")
         SELECT \"hash\", \"bytes\", \"size\" FROM \"blobs_repair\""
            .to_string(),
        Table::drop()
            .table(BlobsRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(Files::Table, FilesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        files_v6_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"files\" (\"site_id\", \"path\", \"hash\", \"size\")
         SELECT \"site_id\", \"path\", \"hash\", \"size\" FROM \"files_repair_source\""
            .to_string(),
        Table::drop()
            .table(FilesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(FilesRepairSource::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("files_hash", Files::Table, [Files::Hash]).to_string(SqliteQueryBuilder),
        index(
            "files_site_prefix",
            Files::Table,
            [Files::SiteId, Files::Path],
        )
        .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoFiles::Table, UndoFilesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_files_table_v10().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_files\" (\"token\", \"path\", \"hash\", \"size\")
         SELECT \"token\", \"path\", \"hash\", \"size\" FROM \"undo_files_repair_source\""
            .to_string(),
        Table::drop()
            .table(UndoFilesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::drop()
            .table(UndoFilesRepairSource::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        index("undo_files_hash", UndoFiles::Table, [UndoFiles::Hash])
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(UndoSites::Table, UndoSitesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        undo_sites_v6_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"undo_sites\"
            (\"token\", \"name\", \"existed\", \"public_url\", \"updated\", \"content_revision\",
             \"tree_hash\")
         SELECT \"token\", \"name\", \"existed\", \"public_url\", \"updated\", \"content_revision\",
                COALESCE(\"tree_hash\", '')
         FROM \"undo_sites_repair\""
            .to_string(),
        Table::drop()
            .table(UndoSitesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        Table::rename()
            .table(PathAggregates::Table, PathAggregatesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
        path_aggregates_table().to_string(SqliteQueryBuilder),
        "INSERT INTO \"path_aggregates\"
            (\"site_id\", \"path\", \"logical_bytes\", \"file_count\")
         SELECT \"site_id\", \"path\", \"logical_bytes\", \"file_count\"
         FROM \"path_aggregates_repair\""
            .to_string(),
        Table::drop()
            .table(PathAggregatesRepair::Table)
            .to_owned()
            .to_string(SqliteQueryBuilder),
    ]
}

fn sites_table() -> TableCreateStatement {
    sites_table_inner(true)
}

fn sites_table_v10() -> TableCreateStatement {
    sites_table_inner(false)
}

fn sites_table_inner(binary_tree_hash: bool) -> TableCreateStatement {
    let mut tree_hash_col = ColumnDef::new(Sites::TreeHash);
    if binary_tree_hash {
        tree_hash_col
            .blob()
            .not_null()
            .default([0_u8; 32].to_vec());
    } else {
        tree_hash_col.text().not_null().default("");
    }
    Table::create()
        .table(Sites::Table)
        .if_not_exists()
        .col(ColumnDef::new(Sites::Id).integer().primary_key())
        .col(ColumnDef::new(Sites::Name).text().not_null().unique_key())
        .col(ColumnDef::new(Sites::Created).integer())
        .col(ColumnDef::new(Sites::Updated).integer().not_null())
        .col(
            ColumnDef::new(Sites::PublicUrl)
                .text()
                .not_null()
                .default(""),
        )
        .col(
            ColumnDef::new(Sites::ContentRevision)
                .integer()
                .not_null()
                .default(0),
        )
        .col(tree_hash_col)
        .col(ColumnDef::new(Sites::CreatorKind).integer())
        .col(ColumnDef::new(Sites::CreatorHash).blob())
        .col(ColumnDef::new(Sites::ClaimHash).blob())
        .col(ColumnDef::new(Sites::ManagementHash).blob())
        .col(
            ColumnDef::new(Sites::ManagementStatus)
                .integer()
                .not_null()
                .default(0),
        )
        .to_owned()
}

fn site_events_table() -> TableCreateStatement {
    Table::create()
        .table(SiteEvents::Table)
        .if_not_exists()
        .col(ColumnDef::new(SiteEvents::Id).integer().primary_key())
        .col(ColumnDef::new(SiteEvents::SiteId).integer().not_null())
        .col(ColumnDef::new(SiteEvents::Kind).integer().not_null())
        .col(ColumnDef::new(SiteEvents::Occurred).integer().not_null())
        .col(
            ColumnDef::new(SiteEvents::Files)
                .integer()
                .not_null()
                .default(0),
        )
        .foreign_key(
            ForeignKey::create()
                .from(SiteEvents::Table, SiteEvents::SiteId)
                .to(Sites::Table, Sites::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn hash_column(iden: impl Iden, binary_hashes: bool) -> ColumnDef {
    let mut col = ColumnDef::new(iden);
    if binary_hashes {
        col.blob();
    } else {
        col.text();
    }
    col
}

fn hash_column_primary_key(iden: impl Iden, binary_hashes: bool) -> ColumnDef {
    let mut col = hash_column(iden, binary_hashes);
    col.primary_key();
    col
}

fn hash_column_not_null(iden: impl Iden, binary_hashes: bool) -> ColumnDef {
    let mut col = hash_column(iden, binary_hashes);
    col.not_null();
    col
}

fn blobs_table() -> TableCreateStatement {
    blobs_table_inner(true)
}

fn blobs_table_v10() -> TableCreateStatement {
    blobs_table_inner(false)
}

fn blobs_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column_primary_key(Blobs::Hash, binary_hashes);
    Table::create()
        .table(Blobs::Table)
        .if_not_exists()
        .col(hash_col)
        .col(
            ColumnDef::new(Blobs::Bytes)
                .blob()
                .not_null()
                .default(Vec::<u8>::new()),
        )
        .col(ColumnDef::new(Blobs::Size).integer().not_null())
        .to_owned()
}

fn files_table() -> TableCreateStatement {
    files_table_inner(true)
}

fn files_table_v10() -> TableCreateStatement {
    files_table_inner(false)
}

fn files_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column_not_null(Files::Hash, binary_hashes);
    Table::create()
        .table(Files::Table)
        .if_not_exists()
        .col(ColumnDef::new(Files::SiteId).integer().not_null())
        .col(ColumnDef::new(Files::Path).text().not_null())
        .col(
            ColumnDef::new(Files::Kind)
                .integer()
                .not_null()
                .default(FILE_ENTRY_KIND)
                .check(Expr::col(Files::Kind).eq(FILE_ENTRY_KIND)),
        )
        .col(hash_col)
        .col(ColumnDef::new(Files::Size).integer().not_null())
        .primary_key(Index::create().col(Files::SiteId).col(Files::Path))
        .foreign_key(
            ForeignKey::create()
                .from_tbl(Files::Table)
                .from_col(Files::SiteId)
                .from_col(Files::Path)
                .from_col(Files::Kind)
                .to_tbl(SiteEntries::Table)
                .to_col(SiteEntries::SiteId)
                .to_col(SiteEntries::Path)
                .to_col(SiteEntries::Kind)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(Files::Table, Files::Hash)
                .to(Blobs::Table, Blobs::Hash),
        )
        .to_owned()
}

fn sites_v6_table() -> TableCreateStatement {
    Table::create()
        .table(Sites::Table)
        .if_not_exists()
        .col(ColumnDef::new(Sites::Id).integer().primary_key())
        .col(ColumnDef::new(Sites::Name).text().not_null().unique_key())
        .col(ColumnDef::new(Sites::Updated).integer().not_null())
        .col(
            ColumnDef::new(Sites::PublicUrl)
                .text()
                .not_null()
                .default(""),
        )
        .col(
            ColumnDef::new(Sites::ContentRevision)
                .integer()
                .not_null()
                .default(0),
        )
        .col(
            ColumnDef::new(Sites::TreeHash)
                .text()
                .not_null()
                .default(""),
        )
        .col(ColumnDef::new(Sites::CreatorKind).integer())
        .col(ColumnDef::new(Sites::CreatorHash).blob())
        .col(ColumnDef::new(Sites::ClaimHash).blob())
        .col(ColumnDef::new(Sites::ManagementHash).blob())
        .col(
            ColumnDef::new(Sites::ManagementStatus)
                .integer()
                .not_null()
                .default(0),
        )
        .to_owned()
}

fn undo_sites_v6_table() -> TableCreateStatement {
    Table::create()
        .table(UndoSites::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoSites::Token).text().primary_key())
        .col(ColumnDef::new(UndoSites::Name).text().not_null())
        .col(ColumnDef::new(UndoSites::Existed).integer().not_null())
        .col(ColumnDef::new(UndoSites::PublicUrl).text().not_null())
        .col(ColumnDef::new(UndoSites::Updated).integer().not_null())
        .col(
            ColumnDef::new(UndoSites::ContentRevision)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(UndoSites::TreeHash).text().not_null())
        .foreign_key(
            ForeignKey::create()
                .from(UndoSites::Table, UndoSites::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn files_v6_table() -> TableCreateStatement {
    Table::create()
        .table(Files::Table)
        .if_not_exists()
        .col(ColumnDef::new(Files::SiteId).integer().not_null())
        .col(ColumnDef::new(Files::Path).text().not_null())
        .col(ColumnDef::new(Files::Hash).text().not_null())
        .col(ColumnDef::new(Files::Size).integer().not_null())
        .primary_key(Index::create().col(Files::SiteId).col(Files::Path))
        .foreign_key(
            ForeignKey::create()
                .from(Files::Table, Files::SiteId)
                .to(Sites::Table, Sites::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(Files::Table, Files::Hash)
                .to(Blobs::Table, Blobs::Hash),
        )
        .to_owned()
}

fn site_entries_table() -> TableCreateStatement {
    Table::create()
        .table(SiteEntries::Table)
        .if_not_exists()
        .col(ColumnDef::new(SiteEntries::SiteId).integer().not_null())
        .col(ColumnDef::new(SiteEntries::Path).text().not_null())
        .col(ColumnDef::new(SiteEntries::Kind).integer().not_null())
        .primary_key(
            Index::create()
                .col(SiteEntries::SiteId)
                .col(SiteEntries::Path),
        )
        .index(
            Index::create()
                .unique()
                .col(SiteEntries::SiteId)
                .col(SiteEntries::Path)
                .col(SiteEntries::Kind),
        )
        .foreign_key(
            ForeignKey::create()
                .from(SiteEntries::Table, SiteEntries::SiteId)
                .to(Sites::Table, Sites::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn metadata_table() -> TableCreateStatement {
    Table::create()
        .table(Metadata::Table)
        .if_not_exists()
        .col(ColumnDef::new(Metadata::Key).text().primary_key())
        .col(ColumnDef::new(Metadata::Value).text().not_null())
        .to_owned()
}

fn undo_operations_table() -> TableCreateStatement {
    Table::create()
        .table(UndoOperations::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoOperations::Token).text().primary_key())
        .col(ColumnDef::new(UndoOperations::Kind).integer().not_null())
        .col(
            ColumnDef::new(UndoOperations::Description)
                .text()
                .not_null(),
        )
        .col(ColumnDef::new(UndoOperations::Created).integer().not_null())
        .col(ColumnDef::new(UndoOperations::Expires).integer().not_null())
        .col(
            ColumnDef::new(UndoOperations::Consumed)
                .integer()
                .not_null()
                .default(0),
        )
        .to_owned()
}

fn undo_names_table() -> TableCreateStatement {
    Table::create()
        .table(UndoNames::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoNames::Token).text().not_null())
        .col(ColumnDef::new(UndoNames::Name).text().not_null())
        .primary_key(Index::create().col(UndoNames::Token).col(UndoNames::Name))
        .foreign_key(
            ForeignKey::create()
                .from(UndoNames::Table, UndoNames::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn undo_sites_table() -> TableCreateStatement {
    undo_sites_table_inner(true)
}

fn undo_sites_table_v10() -> TableCreateStatement {
    undo_sites_table_inner(false)
}

fn undo_sites_table_inner(binary_tree_hash: bool) -> TableCreateStatement {
    let mut tree_hash_col = ColumnDef::new(UndoSites::TreeHash);
    if binary_tree_hash {
        tree_hash_col.blob().not_null();
    } else {
        tree_hash_col.text().not_null().default("");
    }
    Table::create()
        .table(UndoSites::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoSites::Token).text().primary_key())
        .col(ColumnDef::new(UndoSites::Name).text().not_null())
        .col(ColumnDef::new(UndoSites::Existed).integer().not_null())
        .col(ColumnDef::new(UndoSites::PublicUrl).text().not_null())
        .col(ColumnDef::new(UndoSites::Created).integer())
        .col(ColumnDef::new(UndoSites::Updated).integer().not_null())
        .col(
            ColumnDef::new(UndoSites::ContentRevision)
                .integer()
                .not_null(),
        )
        .col(tree_hash_col)
        .foreign_key(
            ForeignKey::create()
                .from(UndoSites::Table, UndoSites::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn undo_files_table() -> TableCreateStatement {
    undo_files_table_inner(true)
}

fn undo_files_table_v10() -> TableCreateStatement {
    undo_files_table_inner(false)
}

fn undo_files_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column_not_null(UndoFiles::Hash, binary_hashes);
    Table::create()
        .table(UndoFiles::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoFiles::Token).text().not_null())
        .col(ColumnDef::new(UndoFiles::Path).text().not_null())
        .col(hash_col)
        .col(ColumnDef::new(UndoFiles::Size).integer().not_null())
        .primary_key(Index::create().col(UndoFiles::Token).col(UndoFiles::Path))
        .foreign_key(
            ForeignKey::create()
                .from(UndoFiles::Table, UndoFiles::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(UndoFiles::Table, UndoFiles::Hash)
                .to(Blobs::Table, Blobs::Hash),
        )
        .to_owned()
}

fn expiry_policies_table() -> TableCreateStatement {
    let mut table = Table::create();
    table
        .table(ExpiryPolicies::Table)
        .if_not_exists()
        .col(ColumnDef::new(ExpiryPolicies::SiteId).integer().not_null())
        .col(ColumnDef::new(ExpiryPolicies::Path).text().not_null())
        .col(
            ColumnDef::new(ExpiryPolicies::TargetKind)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(ExpiryPolicies::Mode).integer().not_null());
    add_expiry_columns(&mut table);
    table
        .primary_key(
            Index::create()
                .col(ExpiryPolicies::SiteId)
                .col(ExpiryPolicies::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(ExpiryPolicies::Table, ExpiryPolicies::SiteId)
                .to(Sites::Table, Sites::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn undo_expiry_policies_table() -> TableCreateStatement {
    let mut table = Table::create();
    table
        .table(UndoExpiryPolicies::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoExpiryPolicies::Token).text().not_null())
        .col(ColumnDef::new(UndoExpiryPolicies::Path).text().not_null())
        .col(
            ColumnDef::new(UndoExpiryPolicies::TargetKind)
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::Mode)
                .integer()
                .not_null(),
        );
    add_undo_expiry_columns(&mut table);
    table
        .primary_key(
            Index::create()
                .col(UndoExpiryPolicies::Token)
                .col(UndoExpiryPolicies::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(UndoExpiryPolicies::Table, UndoExpiryPolicies::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn idempotency_records_table() -> TableCreateStatement {
    Table::create()
        .table(IdempotencyRecords::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(IdempotencyRecords::KeyHash)
                .text()
                .primary_key(),
        )
        .col(
            ColumnDef::new(IdempotencyRecords::Fingerprint)
                .text()
                .not_null(),
        )
        .col(
            ColumnDef::new(IdempotencyRecords::OperationKind)
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(IdempotencyRecords::ResultMetadata)
                .text()
                .not_null(),
        )
        .col(
            ColumnDef::new(IdempotencyRecords::Expires)
                .integer()
                .not_null(),
        )
        .to_owned()
}

fn management_tombstones_table() -> TableCreateStatement {
    Table::create()
        .table(ManagementTombstones::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(ManagementTombstones::Name)
                .text()
                .primary_key(),
        )
        .col(
            ColumnDef::new(ManagementTombstones::ManagementHash)
                .blob()
                .not_null(),
        )
        .col(
            ColumnDef::new(ManagementTombstones::Created)
                .integer()
                .not_null(),
        )
        .to_owned()
}

fn management_audit_table() -> TableCreateStatement {
    Table::create()
        .table(ManagementAudit::Table)
        .if_not_exists()
        .col(ColumnDef::new(ManagementAudit::Id).integer().primary_key())
        .col(ColumnDef::new(ManagementAudit::SiteName).text().not_null())
        .col(ColumnDef::new(ManagementAudit::Action).integer().not_null())
        .col(
            ColumnDef::new(ManagementAudit::Occurred)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(ManagementAudit::SourceIp).text())
        .to_owned()
}

fn management_idempotency_table() -> TableCreateStatement {
    Table::create()
        .table(ManagementIdempotency::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(ManagementIdempotency::KeyHash)
                .text()
                .primary_key(),
        )
        .col(
            ColumnDef::new(ManagementIdempotency::Fingerprint)
                .text()
                .not_null(),
        )
        .col(
            ColumnDef::new(ManagementIdempotency::Expires)
                .integer()
                .not_null(),
        )
        .to_owned()
}

fn path_aggregates_table() -> TableCreateStatement {
    Table::create()
        .table(PathAggregates::Table)
        .if_not_exists()
        .col(ColumnDef::new(PathAggregates::SiteId).integer().not_null())
        .col(ColumnDef::new(PathAggregates::Path).text().not_null())
        .col(
            ColumnDef::new(PathAggregates::LogicalBytes)
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PathAggregates::FileCount)
                .integer()
                .not_null(),
        )
        .primary_key(
            Index::create()
                .col(PathAggregates::SiteId)
                .col(PathAggregates::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(PathAggregates::Table, PathAggregates::SiteId)
                .to(Sites::Table, Sites::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn undo_file_deltas_table() -> TableCreateStatement {
    undo_file_deltas_table_inner(true)
}

fn undo_file_deltas_table_v10() -> TableCreateStatement {
    undo_file_deltas_table_inner(false)
}

fn undo_file_deltas_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column(UndoFileDeltas::Hash, binary_hashes);
    Table::create()
        .table(UndoFileDeltas::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoFileDeltas::Token).text().not_null())
        .col(ColumnDef::new(UndoFileDeltas::Path).text().not_null())
        .col(ColumnDef::new(UndoFileDeltas::Existed).integer().not_null())
        .col(ColumnDef::new(UndoFileDeltas::Kind).integer())
        .col(hash_col)
        .col(ColumnDef::new(UndoFileDeltas::Size).integer())
        .primary_key(
            Index::create()
                .col(UndoFileDeltas::Token)
                .col(UndoFileDeltas::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(UndoFileDeltas::Table, UndoFileDeltas::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn allocated_entries_table() -> TableCreateStatement {
    allocated_entries_table_inner(true)
}

fn allocated_entries_table_v10() -> TableCreateStatement {
    allocated_entries_table_inner(false)
}

fn allocated_entries_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column_not_null(AllocatedEntries::Hash, binary_hashes);
    Table::create()
        .table(AllocatedEntries::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(AllocatedEntries::SiteId)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(AllocatedEntries::Path).text().not_null())
        .col(
            ColumnDef::new(AllocatedEntries::Kind)
                .integer()
                .not_null()
                .default(ALLOCATED_ENTRY_KIND)
                .check(Expr::col(AllocatedEntries::Kind).eq(ALLOCATED_ENTRY_KIND)),
        )
        .col(hash_col)
        .col(ColumnDef::new(AllocatedEntries::Size).integer().not_null())
        .col(
            ColumnDef::new(AllocatedEntries::NamingMode)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(AllocatedEntries::Prefix).text().not_null())
        .col(ColumnDef::new(AllocatedEntries::Suffix).text().not_null())
        .col(ColumnDef::new(AllocatedEntries::Extension).text())
        .col(
            ColumnDef::new(AllocatedEntries::MediaType)
                .text()
                .not_null(),
        )
        .primary_key(
            Index::create()
                .col(AllocatedEntries::SiteId)
                .col(AllocatedEntries::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from_tbl(AllocatedEntries::Table)
                .from_col(AllocatedEntries::SiteId)
                .from_col(AllocatedEntries::Path)
                .from_col(AllocatedEntries::Kind)
                .to_tbl(SiteEntries::Table)
                .to_col(SiteEntries::SiteId)
                .to_col(SiteEntries::Path)
                .to_col(SiteEntries::Kind)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(AllocatedEntries::Table, AllocatedEntries::Hash)
                .to(Blobs::Table, Blobs::Hash),
        )
        .to_owned()
}

fn pending_allocations_table() -> TableCreateStatement {
    pending_allocations_table_inner(true)
}

fn pending_allocations_table_v10() -> TableCreateStatement {
    pending_allocations_table_inner(false)
}

fn pending_allocations_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column_not_null(PendingAllocations::Hash, binary_hashes);
    Table::create()
        .table(PendingAllocations::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(PendingAllocations::Token)
                .text()
                .primary_key(),
        )
        .col(
            ColumnDef::new(PendingAllocations::SiteId)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(PendingAllocations::Folder).text().not_null())
        .col(hash_col)
        .col(
            ColumnDef::new(PendingAllocations::Size)
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PendingAllocations::MediaType)
                .text()
                .not_null(),
        )
        .col(
            ColumnDef::new(PendingAllocations::RequestFingerprint)
                .text()
                .not_null(),
        )
        .col(
            ColumnDef::new(PendingAllocations::Created)
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(PendingAllocations::Expires)
                .integer()
                .not_null(),
        )
        .foreign_key(
            ForeignKey::create()
                .from(PendingAllocations::Table, PendingAllocations::SiteId)
                .to(Sites::Table, Sites::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .foreign_key(
            ForeignKey::create()
                .from(PendingAllocations::Table, PendingAllocations::Hash)
                .to(Blobs::Table, Blobs::Hash),
        )
        .to_owned()
}

fn undo_allocated_deltas_table() -> TableCreateStatement {
    undo_allocated_deltas_table_inner(true)
}

fn undo_allocated_deltas_table_v10() -> TableCreateStatement {
    undo_allocated_deltas_table_inner(false)
}

fn undo_allocated_deltas_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column(UndoAllocatedDeltas::Hash, binary_hashes);
    Table::create()
        .table(UndoAllocatedDeltas::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoAllocatedDeltas::Token).text().not_null())
        .col(ColumnDef::new(UndoAllocatedDeltas::Path).text().not_null())
        .col(
            ColumnDef::new(UndoAllocatedDeltas::Existed)
                .integer()
                .not_null(),
        )
        .col(hash_col)
        .col(ColumnDef::new(UndoAllocatedDeltas::Size).integer())
        .col(ColumnDef::new(UndoAllocatedDeltas::NamingMode).integer())
        .col(ColumnDef::new(UndoAllocatedDeltas::Prefix).text())
        .col(ColumnDef::new(UndoAllocatedDeltas::Suffix).text())
        .col(ColumnDef::new(UndoAllocatedDeltas::Extension).text())
        .col(ColumnDef::new(UndoAllocatedDeltas::MediaType).text())
        .primary_key(
            Index::create()
                .col(UndoAllocatedDeltas::Token)
                .col(UndoAllocatedDeltas::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(UndoAllocatedDeltas::Table, UndoAllocatedDeltas::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn aliases_table() -> TableCreateStatement {
    aliases_table_inner(true)
}

fn aliases_table_v10() -> TableCreateStatement {
    aliases_table_inner(false)
}

fn aliases_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column(Aliases::ResolvedHash, binary_hashes);
    Table::create()
        .table(Aliases::Table)
        .if_not_exists()
        .col(ColumnDef::new(Aliases::SiteId).integer().not_null())
        .col(ColumnDef::new(Aliases::Path).text().not_null())
        .col(
            ColumnDef::new(Aliases::Kind)
                .integer()
                .not_null()
                .default(ALIAS_ENTRY_KIND)
                .check(Expr::col(Aliases::Kind).eq(ALIAS_ENTRY_KIND)),
        )
        .col(ColumnDef::new(Aliases::CanonicalTarget).text().not_null())
        .col(ColumnDef::new(Aliases::ResolvedKind).integer())
        .col(hash_col)
        .col(ColumnDef::new(Aliases::ResolvedSize).integer())
        .primary_key(Index::create().col(Aliases::SiteId).col(Aliases::Path))
        .foreign_key(
            ForeignKey::create()
                .from_tbl(Aliases::Table)
                .from_col(Aliases::SiteId)
                .from_col(Aliases::Path)
                .from_col(Aliases::Kind)
                .to_tbl(SiteEntries::Table)
                .to_col(SiteEntries::SiteId)
                .to_col(SiteEntries::Path)
                .to_col(SiteEntries::Kind)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn undo_alias_deltas_table() -> TableCreateStatement {
    undo_alias_deltas_table_inner(true)
}

fn undo_alias_deltas_table_v10() -> TableCreateStatement {
    undo_alias_deltas_table_inner(false)
}

fn undo_alias_deltas_table_inner(binary_hashes: bool) -> TableCreateStatement {
    let hash_col = hash_column(UndoAliasDeltas::ResolvedHash, binary_hashes);
    Table::create()
        .table(UndoAliasDeltas::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoAliasDeltas::Token).text().not_null())
        .col(ColumnDef::new(UndoAliasDeltas::Path).text().not_null())
        .col(
            ColumnDef::new(UndoAliasDeltas::Existed)
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(UndoAliasDeltas::CanonicalTarget).text())
        .col(ColumnDef::new(UndoAliasDeltas::ResolvedKind).integer())
        .col(hash_col)
        .col(ColumnDef::new(UndoAliasDeltas::ResolvedSize).integer())
        .primary_key(
            Index::create()
                .col(UndoAliasDeltas::Token)
                .col(UndoAliasDeltas::Path),
        )
        .foreign_key(
            ForeignKey::create()
                .from(UndoAliasDeltas::Table, UndoAliasDeltas::Token)
                .to(UndoOperations::Table, UndoOperations::Token)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

fn add_expiry_columns(table: &mut TableCreateStatement) {
    table
        .col(ColumnDef::new(ExpiryPolicies::DurationSeconds).integer())
        .col(ColumnDef::new(ExpiryPolicies::Deadline).integer())
        .col(ColumnDef::new(ExpiryPolicies::MinAgeSeconds).integer())
        .col(ColumnDef::new(ExpiryPolicies::MaxAgeSeconds).integer())
        .col(ColumnDef::new(ExpiryPolicies::MaxSizeBytes).integer())
        .col(ColumnDef::new(ExpiryPolicies::Power).custom(Alias::new("real")))
        .col(ColumnDef::new(ExpiryPolicies::Refreshed).integer())
        .col(ColumnDef::new(ExpiryPolicies::OwnDeadline).integer())
        .col(
            ColumnDef::new(ExpiryPolicies::SizeBytes)
                .integer()
                .not_null()
                .default(0),
        );
}

fn add_undo_expiry_columns(table: &mut TableCreateStatement) {
    table
        .col(
            ColumnDef::new(UndoExpiryPolicies::DurationSeconds)
                .integer()
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::Deadline)
                .integer()
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::MinAgeSeconds)
                .integer()
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::MaxAgeSeconds)
                .integer()
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::MaxSizeBytes)
                .integer()
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::Power)
                .custom(Alias::new("real"))
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::Refreshed)
                .integer()
                .to_owned(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::OwnDeadline)
                .integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(UndoExpiryPolicies::SizeBytes)
                .integer()
                .not_null(),
        );
}

fn index<T, C, const N: usize>(name: &str, table: T, columns: [C; N]) -> IndexCreateStatement
where
    T: IntoTableRef,
    C: IntoIden,
{
    let mut index = Index::create();
    index.name(name).table(table).if_not_exists();
    for column in columns {
        index.col(column);
    }
    index.take()
}

#[cfg(test)]
mod tests {
    use super::schema_sql;

    #[test]
    fn generated_schema_snapshot_is_current() {
        assert_eq!(
            schema_sql(),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema.sql"))
        );
    }
}

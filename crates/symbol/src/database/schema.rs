use std::fmt::Write as _;

#[cfg(test)]
use sea_query::Query;
use sea_query::{
    Alias, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, IntoIden, IntoTableRef, SqliteQueryBuilder, Table, TableCreateStatement,
};

pub const LATEST_SCHEMA_VERSION: i64 = 9;
pub const FILE_ENTRY_KIND: i64 = 0;
pub const ALLOCATED_ENTRY_KIND: i64 = 1;
pub const ALIAS_ENTRY_KIND: i64 = 2;

#[derive(Iden)]
enum Sites {
    Table,
    Id,
    Name,
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
        sites_table(),
        blobs_table(),
        files_v6_table(),
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
        files_table().to_string(SqliteQueryBuilder),
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
        undo_file_deltas_table().to_string(SqliteQueryBuilder),
    ]
}

pub fn upgrade_v7_to_v8() -> Vec<String> {
    vec![
        allocated_entries_table().to_string(SqliteQueryBuilder),
        pending_allocations_table().to_string(SqliteQueryBuilder),
        undo_allocated_deltas_table().to_string(SqliteQueryBuilder),
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
        files_table().to_string(SqliteQueryBuilder),
        allocated_entries_table().to_string(SqliteQueryBuilder),
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
        aliases_table().to_string(SqliteQueryBuilder),
        undo_alias_deltas_table().to_string(SqliteQueryBuilder),
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

fn sites_table() -> TableCreateStatement {
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

fn blobs_table() -> TableCreateStatement {
    Table::create()
        .table(Blobs::Table)
        .if_not_exists()
        .col(ColumnDef::new(Blobs::Hash).text().primary_key())
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
        .col(ColumnDef::new(Files::Hash).text().not_null())
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

fn undo_files_table() -> TableCreateStatement {
    Table::create()
        .table(UndoFiles::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoFiles::Token).text().not_null())
        .col(ColumnDef::new(UndoFiles::Path).text().not_null())
        .col(ColumnDef::new(UndoFiles::Hash).text().not_null())
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
    Table::create()
        .table(UndoFileDeltas::Table)
        .if_not_exists()
        .col(ColumnDef::new(UndoFileDeltas::Token).text().not_null())
        .col(ColumnDef::new(UndoFileDeltas::Path).text().not_null())
        .col(ColumnDef::new(UndoFileDeltas::Existed).integer().not_null())
        .col(ColumnDef::new(UndoFileDeltas::Kind).integer())
        .col(ColumnDef::new(UndoFileDeltas::Hash).text())
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
        .col(ColumnDef::new(AllocatedEntries::Hash).text().not_null())
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
        .col(ColumnDef::new(PendingAllocations::Hash).text().not_null())
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
        .col(ColumnDef::new(UndoAllocatedDeltas::Hash).text())
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
        .col(ColumnDef::new(Aliases::ResolvedHash).text())
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
        .col(ColumnDef::new(UndoAliasDeltas::ResolvedHash).text())
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

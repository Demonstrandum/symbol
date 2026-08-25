use std::fmt::Write as _;

use sea_query::{
    Alias, ColumnDef, ForeignKey, ForeignKeyAction, Iden, Index, IndexCreateStatement, IntoIden,
    IntoTableRef, SqliteQueryBuilder, Table, TableCreateStatement,
};

pub const LATEST_SCHEMA_VERSION: i64 = 6;

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
    Hash,
    Size,
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

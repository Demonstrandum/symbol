use std::fmt::Write as _;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use serde::{Deserialize, Serialize};

use super::catalog::{CatalogDifference, SchemaCatalog};
use super::legacy_schema;
use super::schema;
#[cfg(test)]
use crate::hash::ContentHash;
#[cfg(test)]
use crate::schema::files;
use crate::schema::{metadata, site_entries};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub upgraded_from_v2: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("unsupported database schema version {0}")]
    UnsupportedVersion(i64),
    #[error(transparent)]
    Connection(#[from] diesel::ConnectionError),
    #[error(transparent)]
    Database(#[from] diesel::result::Error),
    #[error("invalid migration metadata: {0}")]
    Metadata(String),
    #[error("database schema v6 catalog drift: {0}")]
    Catalog(#[from] CatalogDifference),
}

#[derive(QueryableByName)]
struct SchemaVersion {
    #[diesel(sql_type = BigInt)]
    user_version: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct MigrationRecord {
    version: i64,
    schema_hash: String,
    program: MigrationProgram,
    program_hash: String,
    source_revision: u64,
    applied_unix_seconds: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum MigrationProgram {
    FreshV6,
    V2ToV6,
    BaselineV6,
    FreshV11,
    V2ToV11,
    V6ToV11,
    V7ToV11,
    V8ToV11,
    V9ToV11,
    V10ToV11,
    BaselineV11,
}

pub fn migrate(db: &mut SqliteConnection) -> Result<MigrationOutcome, MigrationError> {
    let version = schema_version(db)?;
    match version {
        0 => db.transaction::<_, MigrationError, _>(|connection| {
            execute(connection, &schema::schema_sql())?;
            ensure_migration_record(connection, MigrationProgram::FreshV11)
        }),
        2 => db.transaction::<_, MigrationError, _>(|connection| {
            execute(connection, "PRAGMA foreign_keys=OFF")?;
            for statement in schema::upgrade_v2_to_v6() {
                execute(connection, &statement)?;
            }
            for statement in schema::normalize_v6_schema() {
                execute(connection, &statement)?;
            }
            execute(connection, "PRAGMA foreign_keys=ON")?;
            validate_v6_catalog(connection)?;
            migrate_v6_to_latest(connection)?;
            ensure_migration_record(connection, MigrationProgram::V2ToV11)
        }),
        6 => db.transaction::<_, MigrationError, _>(|connection| {
            validate_v6_record(connection)?;
            validate_v6_catalog(connection)?;
            migrate_v6_to_latest(connection)?;
            diesel::delete(metadata::table.find("schema.migration.v6")).execute(connection)?;
            ensure_migration_record(connection, MigrationProgram::V6ToV11)
        }),
        7 => db.transaction::<_, MigrationError, _>(|connection| {
            for statement in schema::upgrade_v7_to_v8()
                .into_iter()
                .chain(schema::upgrade_v8_to_v9())
                .chain(schema::upgrade_v9_to_v10())
                .chain(schema::upgrade_v10_to_v11())
            {
                execute(connection, &statement)?;
            }
            set_schema_version(connection, schema::LATEST_SCHEMA_VERSION)?;
            ensure_migration_record(connection, MigrationProgram::V7ToV11)
        }),
        8 => db.transaction::<_, MigrationError, _>(|connection| {
            for statement in schema::upgrade_v8_to_v9()
                .into_iter()
                .chain(schema::upgrade_v9_to_v10())
                .chain(schema::upgrade_v10_to_v11())
            {
                execute(connection, &statement)?;
            }
            set_schema_version(connection, schema::LATEST_SCHEMA_VERSION)?;
            ensure_migration_record(connection, MigrationProgram::V8ToV11)
        }),
        9 => db.transaction::<_, MigrationError, _>(|connection| {
            for statement in schema::upgrade_v9_to_v10()
                .into_iter()
                .chain(schema::upgrade_v10_to_v11())
            {
                execute(connection, &statement)?;
            }
            diesel::delete(metadata::table.find("schema.migration.v9")).execute(connection)?;
            set_schema_version(connection, schema::LATEST_SCHEMA_VERSION)?;
            ensure_migration_record(connection, MigrationProgram::V9ToV11)
        }),
        10 => db.transaction::<_, MigrationError, _>(|connection| {
            for statement in schema::upgrade_v10_to_v11() {
                execute(connection, &statement)?;
            }
            diesel::delete(metadata::table.find("schema.migration.v11")).execute(connection)?;
            set_schema_version(connection, schema::LATEST_SCHEMA_VERSION)?;
            ensure_migration_record(connection, MigrationProgram::V10ToV11)
        }),
        schema::LATEST_SCHEMA_VERSION => db.transaction::<_, MigrationError, _>(|connection| {
            ensure_migration_record(connection, MigrationProgram::BaselineV11)
        }),
        unsupported => return Err(MigrationError::UnsupportedVersion(unsupported)),
    }?;
    Ok(MigrationOutcome {
        upgraded_from_v2: version == 2,
    })
}

#[derive(Insertable)]
#[diesel(table_name = site_entries)]
struct NewSiteEntry<'a> {
    site_id: i64,
    path: &'a str,
    kind: i64,
}

#[derive(Queryable)]
struct LegacyFile {
    site_id: i64,
    path: String,
    hash: String,
    size: i64,
}

#[derive(Insertable)]
#[diesel(table_name = legacy_schema::files)]
struct MigratedTextHashFile<'a> {
    site_id: i64,
    path: &'a str,
    kind: i64,
    hash: &'a str,
    size: i64,
}

fn migrate_v6_to_latest(db: &mut SqliteConnection) -> Result<(), MigrationError> {
    let legacy_files = legacy_schema::files::table
        .select((
            legacy_schema::files::site_id,
            legacy_schema::files::path,
            legacy_schema::files::hash,
            legacy_schema::files::size,
        ))
        .load::<LegacyFile>(db)?;
    for statement in schema::upgrade_v6_to_v7_before_copy() {
        execute(db, &statement)?;
    }
    for file in &legacy_files {
        diesel::insert_into(site_entries::table)
            .values(NewSiteEntry {
                site_id: file.site_id,
                path: &file.path,
                kind: schema::FILE_ENTRY_KIND,
            })
            .execute(db)?;
    }
    for statement in schema::upgrade_v6_to_v7_after_backfill() {
        execute(db, &statement)?;
    }
    for file in &legacy_files {
        diesel::insert_into(legacy_schema::files::table)
            .values(MigratedTextHashFile {
                site_id: file.site_id,
                path: &file.path,
                kind: schema::FILE_ENTRY_KIND,
                hash: &file.hash,
                size: file.size,
            })
            .execute(db)?;
    }
    for statement in schema::upgrade_v6_to_v7_after_file_copy()
        .into_iter()
        .chain(schema::upgrade_v7_to_v8())
        .chain(schema::upgrade_v8_to_v9())
        .chain(schema::upgrade_v9_to_v10())
        .chain(schema::upgrade_v10_to_v11())
    {
        execute(db, &statement)?;
    }
    set_schema_version(db, schema::LATEST_SCHEMA_VERSION)
}

pub fn schema_version(db: &mut SqliteConnection) -> Result<i64, diesel::result::Error> {
    diesel::sql_query("PRAGMA user_version")
        .get_result::<SchemaVersion>(db)
        .map(|version| version.user_version)
}

#[cfg(test)]
pub fn downgrade_to_v2(db: &mut SqliteConnection) -> Result<(), MigrationError> {
    execute(db, "PRAGMA foreign_keys=OFF")?;
    let files = files::table
        .select((files::site_id, files::path, files::hash, files::size))
        .load::<(i64, String, ContentHash, i64)>(db)?;
    diesel::delete(metadata::table.find("schema.migration.v11")).execute(db)?;
    for statement in schema::downgrade_v11_to_v10() {
        execute(db, &statement)?;
    }
    for statement in schema::downgrade_v9_to_v6_before_copy() {
        execute(db, &statement)?;
    }
    for (site_id, path, hash, size) in &files {
        execute(
            db,
            &schema::insert_v6_file(*site_id, path, &hash.to_hex(), *size),
        )?;
    }
    for statement in schema::downgrade_v9_to_v6_after_copy() {
        execute(db, &statement)?;
    }
    for statement in schema::downgrade_v6_to_v2() {
        execute(db, &statement)?;
    }
    execute(db, "PRAGMA foreign_keys=ON")?;
    set_schema_version(db, 2)
}

fn set_schema_version(db: &mut SqliteConnection, version: i64) -> Result<(), MigrationError> {
    execute(db, &format!("PRAGMA user_version = {version}"))
}

fn execute(db: &mut SqliteConnection, statement: &str) -> Result<(), MigrationError> {
    db.batch_execute(statement)?;
    Ok(())
}

fn ensure_migration_record(
    db: &mut SqliteConnection,
    executed_program: MigrationProgram,
) -> Result<(), MigrationError> {
    const KEY: &str = "schema.migration.v11";
    let hash = blake3::hash(schema::schema_sql().as_bytes())
        .to_hex()
        .to_string();
    let existing = metadata::table
        .find(KEY)
        .select(metadata::value)
        .first::<String>(db)
        .optional()?;
    if let Some(existing) = existing {
        let record: MigrationRecord = serde_json::from_str(&existing)
            .map_err(|error| MigrationError::Metadata(error.to_string()))?;
        let expected_program_hash = migration_program_hash(record.program);
        if record.version != schema::LATEST_SCHEMA_VERSION
            || record.schema_hash != hash
            || record.program_hash != expected_program_hash
        {
            return Err(MigrationError::Metadata(format!(
                "schema checksum drift for version {}",
                record.version
            )));
        }
        return Ok(());
    }
    let applied_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| MigrationError::Metadata(error.to_string()))?
        .as_secs();
    let record = MigrationRecord {
        version: schema::LATEST_SCHEMA_VERSION,
        schema_hash: hash,
        program: executed_program,
        program_hash: migration_program_hash(executed_program),
        source_revision: 0,
        applied_unix_seconds,
    };
    let value = serde_json::to_string(&record)
        .map_err(|error| MigrationError::Metadata(error.to_string()))?;
    diesel::insert_into(metadata::table)
        .values((metadata::key.eq(KEY), metadata::value.eq(value)))
        .execute(db)?;
    Ok(())
}

fn validate_v6_record(db: &mut SqliteConnection) -> Result<(), MigrationError> {
    const KEY: &str = "schema.migration.v6";
    let Some(existing) = metadata::table
        .find(KEY)
        .select(metadata::value)
        .first::<String>(db)
        .optional()?
    else {
        return Ok(());
    };
    let record: MigrationRecord = serde_json::from_str(&existing)
        .map_err(|error| MigrationError::Metadata(error.to_string()))?;
    let schema_hash = blake3::hash(schema::schema_v6_sql().as_bytes())
        .to_hex()
        .to_string();
    if record.version != 6
        || record.schema_hash != schema_hash
        || record.program_hash != migration_program_hash(record.program)
        || !matches!(
            record.program,
            MigrationProgram::FreshV6 | MigrationProgram::V2ToV6 | MigrationProgram::BaselineV6
        )
    {
        return Err(MigrationError::Metadata(
            "schema checksum drift for version 6".to_string(),
        ));
    }
    Ok(())
}

fn validate_v6_catalog(db: &mut SqliteConnection) -> Result<(), MigrationError> {
    let mut expected_db = SqliteConnection::establish(":memory:")?;
    execute(&mut expected_db, &schema::schema_v6_sql())?;
    let expected = SchemaCatalog::load(&mut expected_db)?;
    let actual = SchemaCatalog::load(db)?;
    if let Some(difference) = expected.difference(&actual) {
        return Err(difference.into());
    }
    Ok(())
}

fn migration_program_hash(program: MigrationProgram) -> String {
    let source = match program {
        MigrationProgram::FreshV6 => schema::schema_v6_sql(),
        MigrationProgram::V2ToV6 => {
            let mut source = schema::upgrade_v2_to_v6().join(";\n");
            write!(source, ";\nPRAGMA user_version = 6").expect("writing to String cannot fail");
            source
        }
        MigrationProgram::BaselineV6 => {
            format!("baseline-v6\n{}", schema::schema_v6_sql())
        }
        MigrationProgram::FreshV11 => schema::schema_sql(),
        MigrationProgram::V2ToV11 => {
            let mut source = schema::upgrade_v2_to_v6().join(";\n");
            append_v7_to_v11_program(&mut source);
            source
        }
        MigrationProgram::V6ToV11 => {
            let mut source = String::from("validated-v6");
            append_v7_to_v11_program(&mut source);
            source
        }
        MigrationProgram::V7ToV11 => {
            let mut source = String::from("baseline-v7");
            for statement in schema::upgrade_v7_to_v8()
                .into_iter()
                .chain(schema::upgrade_v8_to_v9())
                .chain(schema::upgrade_v9_to_v10())
                .chain(schema::upgrade_v10_to_v11())
            {
                write!(source, ";\n{statement}").expect("writing to String cannot fail");
            }
            write!(
                source,
                ";\nPRAGMA user_version = {}",
                schema::LATEST_SCHEMA_VERSION
            )
            .expect("writing to String cannot fail");
            source
        }
        MigrationProgram::V8ToV11 => {
            let mut source = String::from("baseline-v8");
            for statement in schema::upgrade_v8_to_v9()
                .into_iter()
                .chain(schema::upgrade_v9_to_v10())
                .chain(schema::upgrade_v10_to_v11())
            {
                write!(source, ";\n{statement}").expect("writing to String cannot fail");
            }
            write!(
                source,
                ";\nPRAGMA user_version = {}",
                schema::LATEST_SCHEMA_VERSION
            )
            .expect("writing to String cannot fail");
            source
        }
        MigrationProgram::V9ToV11 => {
            let mut source = String::from("baseline-v9");
            for statement in schema::upgrade_v9_to_v10()
                .into_iter()
                .chain(schema::upgrade_v10_to_v11())
            {
                write!(source, ";\n{statement}").expect("writing to String cannot fail");
            }
            write!(
                source,
                ";\nPRAGMA user_version = {}",
                schema::LATEST_SCHEMA_VERSION
            )
            .expect("writing to String cannot fail");
            source
        }
        MigrationProgram::V10ToV11 => {
            let mut source = String::from("baseline-v10");
            for statement in schema::upgrade_v10_to_v11() {
                write!(source, ";\n{statement}").expect("writing to String cannot fail");
            }
            write!(
                source,
                ";\nPRAGMA user_version = {}",
                schema::LATEST_SCHEMA_VERSION
            )
            .expect("writing to String cannot fail");
            source
        }
        MigrationProgram::BaselineV11 => {
            format!("baseline-v11\n{}", schema::schema_sql())
        }
    };
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

fn append_v7_to_v11_program(source: &mut String) {
    for statement in schema::upgrade_v6_to_v7_before_copy()
        .into_iter()
        .chain(schema::upgrade_v6_to_v7_after_backfill())
        .chain(schema::upgrade_v6_to_v7_after_file_copy())
        .chain(schema::upgrade_v7_to_v8())
        .chain(schema::upgrade_v8_to_v9())
        .chain(schema::upgrade_v9_to_v10())
        .chain(schema::upgrade_v10_to_v11())
    {
        write!(source, ";\n{statement}").expect("writing to String cannot fail");
    }
    write!(
        source,
        ";\nPRAGMA user_version = {}",
        schema::LATEST_SCHEMA_VERSION
    )
    .expect("writing to String cannot fail");
}

#[cfg(test)]
mod tests {
    use diesel::Connection;
    use diesel::dsl::count_star;
    use diesel::sql_types::{BigInt, Text};

    use super::*;
    use crate::database::catalog::SchemaCatalog;
    use crate::hash::{ContentHash, TreeHash};
    use crate::schema::{blobs, files, sites};

    #[derive(Insertable)]
    #[diesel(table_name = files)]
    struct MigratedFile {
        site_id: i64,
        path: &'static str,
        kind: i64,
        hash: ContentHash,
        size: i64,
    }

    fn test_content_hash(label: &str) -> ContentHash {
        ContentHash::from(blake3::hash(label.as_bytes()))
    }

    fn test_tree_hash(label: &str) -> TreeHash {
        TreeHash::from(blake3::hash(label.as_bytes()))
    }

    fn label_hex(label: &str) -> String {
        blake3::hash(label.as_bytes()).to_hex().to_string()
    }

    fn label_tree_wire(label: &str) -> String {
        format!("blake3:{}", label_hex(label))
    }

    // Restored byte-for-byte from the SQL assets removed by e657598.
    const HISTORICAL_SCHEMA_V6: &str =
        include_str!("../../tests/fixtures/historical_schema_v6.sql");
    const HISTORICAL_V6_TO_V2: &str = include_str!("../../tests/fixtures/historical_v6_to_v2.sql");

    #[derive(QueryableByName)]
    struct SchemaObject {
        #[diesel(column_name = "type")]
        #[diesel(sql_type = Text)]
        object_type: String,
        #[diesel(sql_type = Text)]
        name: String,
        #[diesel(sql_type = Text)]
        tbl_name: String,
    }

    #[derive(QueryableByName, Debug, PartialEq, Eq)]
    struct ForeignKeyColumn {
        #[diesel(sql_type = Text)]
        table: String,
        #[diesel(sql_type = Text)]
        from: String,
        #[diesel(sql_type = Text)]
        to: String,
    }

    #[derive(QueryableByName)]
    struct ObjectCount {
        #[diesel(sql_type = BigInt)]
        count: i64,
    }

    #[derive(QueryableByName)]
    struct TableColumn {
        #[diesel(sql_type = Text)]
        name: String,
    }

    fn connection() -> (tempfile::TempDir, SqliteConnection) {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("symbol.db");
        let connection = SqliteConnection::establish(&database.to_string_lossy()).unwrap();
        (root, connection)
    }

    fn execute_v6_sql(db: &mut SqliteConnection, sql: &str) {
        db.batch_execute(sql).unwrap();
        assert_eq!(schema_version(db).unwrap(), 6);
    }

    #[test]
    fn historical_schema_fixtures_remain_byte_exact() {
        assert_eq!(
            blake3::hash(HISTORICAL_SCHEMA_V6.as_bytes())
                .to_hex()
                .as_str(),
            "8617a22b6926034a034f978da702cf7e96c5513f96f9999bc0d885c965cf80a4"
        );
        assert_eq!(
            blake3::hash(HISTORICAL_V6_TO_V2.as_bytes())
                .to_hex()
                .as_str(),
            "1e4a8a077621e0ca6ecccf2afbfdd3ebfb15eecad1374bf460d635a9cb8d72da"
        );
    }

    fn replace_once(source: &str, from: &str, to: &str) -> String {
        assert_eq!(source.matches(from).count(), 1, "{from}");
        source.replacen(from, to, 1)
    }

    #[derive(Debug, Clone, Copy)]
    enum V6CatalogDrift {
        Table,
        Column,
        Type,
        Nullability,
        Default,
        PrimaryKey,
        ForeignKey,
        Index,
        CheckConstraint,
    }

    impl V6CatalogDrift {
        const ALL: [Self; 9] = [
            Self::Table,
            Self::Column,
            Self::Type,
            Self::Nullability,
            Self::Default,
            Self::PrimaryKey,
            Self::ForeignKey,
            Self::Index,
            Self::CheckConstraint,
        ];

        fn initialize(self, db: &mut SqliteConnection) {
            let base = schema::schema_v6_sql();
            let sql = match self {
                Self::Type => replace_once(
                    &base,
                    "\"name\" text NOT NULL UNIQUE, \"updated\" integer NOT NULL",
                    "\"name\" text NOT NULL UNIQUE, \"updated\" text NOT NULL",
                ),
                Self::Nullability => replace_once(
                    &base,
                    "\"name\" text NOT NULL UNIQUE, \"updated\" integer NOT NULL",
                    "\"name\" text NOT NULL UNIQUE, \"updated\" integer",
                ),
                Self::Default => replace_once(
                    &base,
                    "\"public_url\" text NOT NULL DEFAULT ''",
                    "\"public_url\" text NOT NULL DEFAULT 'drift'",
                ),
                Self::PrimaryKey => replace_once(&base, "\"key\" text PRIMARY KEY", "\"key\" text"),
                Self::ForeignKey => replace_once(
                    &base,
                    "FOREIGN KEY (\"site_id\") REFERENCES \"sites\" (\"id\") ON DELETE CASCADE, FOREIGN KEY (\"hash\") REFERENCES \"blobs\" (\"hash\")",
                    "FOREIGN KEY (\"site_id\") REFERENCES \"sites\" (\"id\"), FOREIGN KEY (\"hash\") REFERENCES \"blobs\" (\"hash\")",
                ),
                Self::CheckConstraint => replace_once(
                    &base,
                    "\"size\" integer NOT NULL );",
                    "\"size\" integer NOT NULL CHECK (\"size\" >= 0) );",
                ),
                Self::Table | Self::Column | Self::Index => base,
            };
            execute_v6_sql(db, &sql);
            match self {
                Self::Table => db.batch_execute("DROP TABLE path_aggregates").unwrap(),
                Self::Column => db
                    .batch_execute("ALTER TABLE sites DROP COLUMN creator_kind")
                    .unwrap(),
                Self::Index => db.batch_execute("DROP INDEX files_hash").unwrap(),
                Self::Type
                | Self::Nullability
                | Self::Default
                | Self::PrimaryKey
                | Self::ForeignKey
                | Self::CheckConstraint => {}
            }
        }
    }

    #[test]
    fn rust_v6_catalogs_match_the_historical_sql_contract_in_every_dimension() {
        let (_historical_root, mut historical) = connection();
        execute_v6_sql(&mut historical, HISTORICAL_SCHEMA_V6);
        let expected = SchemaCatalog::load(&mut historical).unwrap();

        let (_fresh_root, mut fresh) = connection();
        execute_v6_sql(&mut fresh, &schema::schema_v6_sql());
        assert_eq!(SchemaCatalog::load(&mut fresh).unwrap(), expected);

        let (_upgrade_root, mut upgraded) = connection();
        execute_v6_sql(&mut upgraded, HISTORICAL_SCHEMA_V6);
        upgraded.batch_execute(HISTORICAL_V6_TO_V2).unwrap();
        assert_eq!(schema_version(&mut upgraded).unwrap(), 2);
        for statement in schema::upgrade_v2_to_v6() {
            execute(&mut upgraded, &statement).unwrap();
        }
        set_schema_version(&mut upgraded, 6).unwrap();
        assert_eq!(SchemaCatalog::load(&mut upgraded).unwrap(), expected);
    }

    #[test]
    fn deployed_legacy_v6_catalog_upgrades_without_weakening_other_dimensions() {
        let (_root, mut db) = connection();
        let legacy_schema = schema::schema_v6_sql().replace(
            "\"bytes\" blob NOT NULL DEFAULT x''",
            "\"bytes\" blob NOT NULL",
        );
        execute_v6_sql(&mut db, &legacy_schema);
        db.batch_execute(
            "CREATE TABLE __diesel_schema_migrations (
                version VARCHAR(50) PRIMARY KEY NOT NULL,
                run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO __diesel_schema_migrations (version)
            VALUES ('00000000000000');",
        )
        .unwrap();

        migrate(&mut db).unwrap();

        assert_eq!(
            schema_version(&mut db).unwrap(),
            schema::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            diesel::sql_query("SELECT COUNT(*) AS count FROM __diesel_schema_migrations")
                .get_result::<ObjectCount>(&mut db)
                .unwrap()
                .count,
            1
        );
    }

    #[test]
    fn untrusted_v6_catalog_drift_is_rejected_without_changes() {
        for drift in V6CatalogDrift::ALL {
            let (_root, mut db) = connection();
            drift.initialize(&mut db);
            diesel::insert_into(metadata::table)
                .values((
                    metadata::key.eq("catalog-validation-probe"),
                    metadata::value.eq("unchanged"),
                ))
                .execute(&mut db)
                .unwrap();
            let before = SchemaCatalog::load(&mut db).unwrap();

            assert!(
                matches!(migrate(&mut db), Err(MigrationError::Catalog(_))),
                "{drift:?} drift was accepted"
            );

            assert_eq!(schema_version(&mut db).unwrap(), 6, "{drift:?}");
            assert_eq!(SchemaCatalog::load(&mut db).unwrap(), before, "{drift:?}");
            assert_eq!(
                metadata::table
                    .find("catalog-validation-probe")
                    .select(metadata::value)
                    .first::<String>(&mut db)
                    .unwrap(),
                "unchanged",
                "{drift:?}"
            );
        }
    }

    #[test]
    fn valid_v6_metadata_does_not_mask_later_catalog_drift() {
        let (_root, mut db) = connection();
        execute_v6_sql(&mut db, &schema::schema_v6_sql());
        let program = MigrationProgram::BaselineV6;
        let record = MigrationRecord {
            version: 6,
            schema_hash: blake3::hash(schema::schema_v6_sql().as_bytes())
                .to_hex()
                .to_string(),
            program,
            program_hash: migration_program_hash(program),
            source_revision: 0,
            applied_unix_seconds: 0,
        };
        diesel::insert_into(metadata::table)
            .values((
                metadata::key.eq("schema.migration.v6"),
                metadata::value.eq(serde_json::to_string(&record).unwrap()),
            ))
            .execute(&mut db)
            .unwrap();
        db.batch_execute("DROP INDEX files_hash").unwrap();
        let before = SchemaCatalog::load(&mut db).unwrap();

        assert!(matches!(migrate(&mut db), Err(MigrationError::Catalog(_))));

        assert_eq!(schema_version(&mut db).unwrap(), 6);
        assert_eq!(SchemaCatalog::load(&mut db).unwrap(), before);
        assert!(
            metadata::table
                .find("schema.migration.v6")
                .select(metadata::value)
                .first::<String>(&mut db)
                .is_ok()
        );
    }

    #[test]
    fn fresh_catalog_has_exact_tables_and_indexes() {
        let (_root, mut db) = connection();
        migrate(&mut db).unwrap();
        let objects = diesel::sql_query(
            "SELECT type, name, tbl_name
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .load::<SchemaObject>(&mut db)
        .unwrap()
        .into_iter()
        .map(|object| (object.object_type, object.name, object.tbl_name))
        .collect::<Vec<_>>();
        let expected = [
            ("index", "aliases_cache", "aliases"),
            ("index", "aliases_dependency", "aliases"),
            ("index", "expiry_policies_deadline", "expiry_policies"),
            ("index", "expiry_policies_site_kind", "expiry_policies"),
            ("index", "files_hash", "files"),
            ("index", "files_site_hash", "files"),
            ("index", "files_site_prefix", "files"),
            ("index", "idempotency_records_expiry", "idempotency_records"),
            ("index", "management_audit_site", "management_audit"),
            (
                "index",
                "management_idempotency_expiry",
                "management_idempotency",
            ),
            ("index", "pending_allocations_expiry", "pending_allocations"),
            ("index", "site_events_site", "site_events"),
            ("index", "undo_files_hash", "undo_files"),
            ("index", "undo_names_stack", "undo_names"),
            ("index", "undo_operations_retention", "undo_operations"),
            ("table", "aliases", "aliases"),
            ("table", "allocated_entries", "allocated_entries"),
            ("table", "blobs", "blobs"),
            ("table", "expiry_policies", "expiry_policies"),
            ("table", "files", "files"),
            ("table", "idempotency_records", "idempotency_records"),
            ("table", "management_audit", "management_audit"),
            ("table", "management_idempotency", "management_idempotency"),
            ("table", "management_tombstones", "management_tombstones"),
            ("table", "metadata", "metadata"),
            ("table", "path_aggregates", "path_aggregates"),
            ("table", "pending_allocations", "pending_allocations"),
            ("table", "site_entries", "site_entries"),
            ("table", "site_events", "site_events"),
            ("table", "sites", "sites"),
            ("table", "undo_alias_deltas", "undo_alias_deltas"),
            ("table", "undo_allocated_deltas", "undo_allocated_deltas"),
            ("table", "undo_expiry_policies", "undo_expiry_policies"),
            ("table", "undo_file_deltas", "undo_file_deltas"),
            ("table", "undo_files", "undo_files"),
            ("table", "undo_names", "undo_names"),
            ("table", "undo_operations", "undo_operations"),
            ("table", "undo_sites", "undo_sites"),
        ]
        .map(|(object_type, name, table)| {
            (object_type.to_string(), name.to_string(), table.to_string())
        });
        assert_eq!(objects, expected);
        assert_eq!(
            schema_version(&mut db).unwrap(),
            schema::LATEST_SCHEMA_VERSION
        );
        let file_foreign_keys = diesel::sql_query(
            "SELECT \"table\", \"from\", \"to\" FROM pragma_foreign_key_list('files')
             ORDER BY id DESC, seq",
        )
        .load::<ForeignKeyColumn>(&mut db)
        .unwrap();
        assert_eq!(
            file_foreign_keys,
            [
                ForeignKeyColumn {
                    table: "site_entries".to_string(),
                    from: "site_id".to_string(),
                    to: "site_id".to_string(),
                },
                ForeignKeyColumn {
                    table: "site_entries".to_string(),
                    from: "path".to_string(),
                    to: "path".to_string(),
                },
                ForeignKeyColumn {
                    table: "site_entries".to_string(),
                    from: "kind".to_string(),
                    to: "kind".to_string(),
                },
                ForeignKeyColumn {
                    table: "blobs".to_string(),
                    from: "hash".to_string(),
                    to: "hash".to_string(),
                },
            ]
        );
    }

    fn seed_v6_file(db: &mut SqliteConnection) {
        let hash = label_hex("legacy-hash");
        let tree_hash = label_tree_wire("legacy-tree");
        execute(db, &schema::schema_v6_sql()).unwrap();
        db.batch_execute(&format!(
            "INSERT INTO sites
                (name, updated, public_url, content_revision, tree_hash, management_status)
             VALUES ('legacy', 17, 'https://symbol.example', 4, '{tree_hash}', 0);
             INSERT INTO blobs (hash, bytes, size) VALUES ('{hash}', X'', 23);"
        ))
        .unwrap();
        let site_id = sites::table.select(sites::id).first::<i64>(db).unwrap();
        execute(
            db,
            &schema::insert_v6_file(site_id, "index.html", &hash, 23),
        )
        .unwrap();
    }

    fn empty_catalog_at(db: &mut SqliteConnection, version: i64) {
        execute(db, &schema::schema_v6_sql()).unwrap();
        for statement in schema::upgrade_v6_to_v7_before_copy()
            .into_iter()
            .chain(schema::upgrade_v6_to_v7_after_backfill())
            .chain(schema::upgrade_v6_to_v7_after_file_copy())
        {
            execute(db, &statement).unwrap();
        }
        if version == 8 {
            for statement in schema::upgrade_v7_to_v8() {
                execute(db, &statement).unwrap();
            }
        }
        set_schema_version(db, version).unwrap();
    }

    #[test]
    fn v7_and_v8_catalogs_migrate_onward() {
        for version in [7, 8] {
            let (_root, mut db) = connection();
            let upgrade_hash = label_hex("upgrade-hash");
            empty_catalog_at(&mut db, version);
            db.batch_execute(&format!(
                "INSERT INTO sites
                    (name, updated, public_url, content_revision, tree_hash, management_status)
                 VALUES ('upgrade', 0, '', 0, '', 0);
                 INSERT INTO blobs (hash, bytes, size) VALUES ('{upgrade_hash}', X'01', 1);
                 INSERT INTO site_entries (site_id, path, kind)
                 SELECT id, 'regular', 0 FROM sites WHERE name = 'upgrade';
                 INSERT INTO files (site_id, path, kind, hash, size)
                 SELECT id, 'regular', 0, '{upgrade_hash}', 1
                 FROM sites WHERE name = 'upgrade';"
            ))
            .unwrap();
            if version == 8 {
                db.batch_execute(&format!(
                    "INSERT INTO site_entries (site_id, path, kind)
                     SELECT id, 'allocated', 1 FROM sites WHERE name = 'upgrade';
                     INSERT INTO allocated_entries
                        (site_id, path, kind, hash, size, naming_mode, prefix, suffix, media_type)
                     SELECT id, 'allocated', 1, '{upgrade_hash}', 1, 0, '', '', ''
                     FROM sites WHERE name = 'upgrade';"
                ))
                .unwrap();
            }

            migrate(&mut db).unwrap();

            assert_eq!(
                schema_version(&mut db).unwrap(),
                schema::LATEST_SCHEMA_VERSION
            );
            let record = metadata::table
                .find("schema.migration.v11")
                .select(metadata::value)
                .first::<String>(&mut db)
                .unwrap();
            let record: MigrationRecord = serde_json::from_str(&record).unwrap();
            assert_eq!(
                record.program,
                if version == 7 {
                    MigrationProgram::V7ToV11
                } else {
                    MigrationProgram::V8ToV11
                }
            );
            assert_eq!(
                diesel::sql_query(
                    "SELECT COUNT(*) AS count FROM sqlite_schema
                     WHERE type = 'table' AND name IN ('allocated_entries', 'aliases')",
                )
                .get_result::<ObjectCount>(&mut db)
                .unwrap()
                .count,
                2
            );
            assert_eq!(
                files::table
                    .select(count_star())
                    .first::<i64>(&mut db)
                    .unwrap(),
                1
            );
            if version == 8 {
                assert_eq!(
                    diesel::sql_query(
                        "SELECT COUNT(*) AS count FROM allocated_entries WHERE path = 'allocated'",
                    )
                    .get_result::<ObjectCount>(&mut db)
                    .unwrap()
                    .count,
                    1
                );
            }
        }
    }

    #[test]
    fn v6_backfill_preserves_files_and_site_data() {
        let (_root, mut db) = connection();
        seed_v6_file(&mut db);
        migrate(&mut db).unwrap();
        assert_eq!(
            files::table
                .select((files::path, files::hash, files::size))
                .first::<(String, ContentHash, i64)>(&mut db)
                .unwrap(),
            (
                "index.html".to_string(),
                test_content_hash("legacy-hash"),
                23
            )
        );
        assert_eq!(
            site_entries::table
                .select((site_entries::path, site_entries::kind,))
                .first::<(String, i64)>(&mut db)
                .unwrap(),
            ("index.html".to_string(), schema::FILE_ENTRY_KIND)
        );
        assert_eq!(
            sites::table
                .select((sites::updated, sites::content_revision, sites::tree_hash))
                .first::<(i64, i64, TreeHash)>(&mut db)
                .unwrap(),
            (17, 4, test_tree_hash("legacy-tree"))
        );
    }

    #[test]
    #[expect(clippy::too_many_lines)]
    fn v2_upgrade_reaches_v9_and_preserves_legacy_files() {
        let (_root, mut db) = connection();
        migrate(&mut db).unwrap();
        diesel::insert_into(sites::table)
            .values((
                sites::name.eq("legacy-v2"),
                sites::updated.eq(9_i64),
                sites::public_url.eq(""),
                sites::content_revision.eq(2_i64),
                sites::tree_hash.eq(test_tree_hash("v2-tree")),
                sites::management_status.eq(0_i64),
            ))
            .execute(&mut db)
            .unwrap();
        let site_id = sites::table
            .select(sites::id)
            .first::<i64>(&mut db)
            .unwrap();
        diesel::insert_into(blobs::table)
            .values((
                blobs::hash.eq(test_content_hash("v2-hash")),
                blobs::bytes.eq(Vec::<u8>::new()),
                blobs::size.eq(7_i64),
            ))
            .execute(&mut db)
            .unwrap();
        diesel::insert_into(site_entries::table)
            .values(NewSiteEntry {
                site_id,
                path: "legacy.txt",
                kind: schema::FILE_ENTRY_KIND,
            })
            .execute(&mut db)
            .unwrap();
        diesel::insert_into(files::table)
            .values(MigratedFile {
                site_id,
                path: "legacy.txt",
                kind: schema::FILE_ENTRY_KIND,
                hash: test_content_hash("v2-hash"),
                size: 7,
            })
            .execute(&mut db)
            .unwrap();
        downgrade_to_v2(&mut db).unwrap();
        assert_eq!(
            files::table.count().get_result::<i64>(&mut db).unwrap(),
            1,
            "downgrade must preserve seeded files in v6 catalog"
        );

        let outcome = migrate(&mut db).unwrap();

        assert!(outcome.upgraded_from_v2);
        assert_eq!(
            schema_version(&mut db).unwrap(),
            schema::LATEST_SCHEMA_VERSION
        );
        let allocated_columns =
            diesel::sql_query("SELECT name FROM pragma_table_info('allocated_entries')")
                .load::<TableColumn>(&mut db)
                .unwrap()
                .into_iter()
                .map(|column| column.name)
                .collect::<Vec<_>>();
        assert_eq!(
            allocated_columns,
            [
                "site_id",
                "path",
                "kind",
                "hash",
                "size",
                "naming_mode",
                "prefix",
                "suffix",
                "extension",
                "media_type",
            ]
        );
        let pending_columns =
            diesel::sql_query("SELECT name FROM pragma_table_info('pending_allocations')")
                .load::<TableColumn>(&mut db)
                .unwrap()
                .into_iter()
                .map(|column| column.name)
                .collect::<Vec<_>>();
        assert_eq!(
            pending_columns,
            [
                "token",
                "site_id",
                "folder",
                "hash",
                "size",
                "media_type",
                "request_fingerprint",
                "created",
                "expires",
            ]
        );
        assert_eq!(
            files::table
                .select((files::path, files::hash, files::size))
                .first::<(String, ContentHash, i64)>(&mut db)
                .unwrap(),
            ("legacy.txt".to_string(), test_content_hash("v2-hash"), 7)
        );
    }

    #[test]
    fn failed_v6_upgrade_rolls_back_all_catalog_changes() {
        let (_root, mut db) = connection();
        execute(&mut db, &schema::schema_v6_sql()).unwrap();
        let before = SchemaCatalog::load(&mut db).unwrap();
        db.batch_execute("PRAGMA foreign_keys = OFF").unwrap();
        execute(
            &mut db,
            &schema::insert_v6_file(999, "orphan", "missing", 1),
        )
        .unwrap();
        db.batch_execute("PRAGMA foreign_keys = ON").unwrap();
        assert!(matches!(migrate(&mut db), Err(MigrationError::Database(_))));
        assert_eq!(schema_version(&mut db).unwrap(), 6);
        let count = diesel::sql_query(
            "SELECT COUNT(*) AS count FROM sqlite_schema WHERE name = 'site_entries'",
        )
        .get_result::<ObjectCount>(&mut db)
        .unwrap()
        .count;
        assert_eq!(count, 0);
        assert_eq!(SchemaCatalog::load(&mut db).unwrap(), before);
        assert!(
            metadata::table
                .find("schema.migration.v9")
                .select(metadata::value)
                .first::<String>(&mut db)
                .optional()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn injected_v2_upgrade_failure_rolls_back_v6_catalog_creation() {
        let (_root, mut db) = connection();
        execute_v6_sql(&mut db, HISTORICAL_SCHEMA_V6);
        db.batch_execute(HISTORICAL_V6_TO_V2).unwrap();
        db.batch_execute("PRAGMA foreign_keys = OFF").unwrap();
        execute(
            &mut db,
            &schema::insert_v6_file(999, "orphan", "missing", 1),
        )
        .unwrap();
        db.batch_execute("PRAGMA foreign_keys = ON").unwrap();
        let before = SchemaCatalog::load(&mut db).unwrap();

        assert!(matches!(migrate(&mut db), Err(MigrationError::Database(_))));

        assert_eq!(schema_version(&mut db).unwrap(), 2);
        assert_eq!(SchemaCatalog::load(&mut db).unwrap(), before);
        assert!(
            metadata::table
                .find("schema.migration.v9")
                .select(metadata::value)
                .first::<String>(&mut db)
                .optional()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn composite_entry_foreign_key_rejects_cross_kind_rows() {
        let (_root, mut db) = connection();
        migrate(&mut db).unwrap();
        diesel::insert_into(sites::table)
            .values((
                sites::name.eq("kinds"),
                sites::updated.eq(0_i64),
                sites::public_url.eq(""),
                sites::content_revision.eq(0_i64),
                sites::tree_hash.eq(TreeHash::EMPTY),
                sites::management_status.eq(0_i64),
            ))
            .execute(&mut db)
            .unwrap();
        let site_id = sites::table
            .select(sites::id)
            .first::<i64>(&mut db)
            .unwrap();
        diesel::insert_into(blobs::table)
            .values((
                blobs::hash.eq(test_content_hash("hash")),
                blobs::bytes.eq(Vec::<u8>::new()),
                blobs::size.eq(1_i64),
            ))
            .execute(&mut db)
            .unwrap();
        diesel::insert_into(site_entries::table)
            .values((
                site_entries::site_id.eq(site_id),
                site_entries::path.eq("reserved"),
                site_entries::kind.eq(schema::ALLOCATED_ENTRY_KIND),
            ))
            .execute(&mut db)
            .unwrap();
        assert!(matches!(
            diesel::insert_into(files::table)
                .values(MigratedFile {
                    site_id,
                    path: "reserved",
                    kind: schema::FILE_ENTRY_KIND,
                    hash: test_content_hash("hash"),
                    size: 1,
                })
                .execute(&mut db),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::ForeignKeyViolation,
                _
            ))
        ));
    }

    #[test]
    fn subtype_tables_reject_noncanonical_kinds() {
        for version in [0, 7, 8] {
            let (_root, mut db) = connection();
            if version != 0 {
                empty_catalog_at(&mut db, version);
            }
            migrate(&mut db).unwrap();
            db.batch_execute(
                "INSERT INTO sites
                    (name, updated, public_url, content_revision, tree_hash, management_status)
                 VALUES ('checks', 0, '', 0, '', 0);
                 INSERT INTO blobs (hash, bytes, size) VALUES ('check-hash', X'', 0);
                 INSERT INTO site_entries (site_id, path, kind)
                 SELECT id, 'bad-file', 1 FROM sites WHERE name = 'checks';
                 INSERT INTO site_entries (site_id, path, kind)
                 SELECT id, 'bad-allocated', 0 FROM sites WHERE name = 'checks';
                 INSERT INTO site_entries (site_id, path, kind)
                 SELECT id, 'bad-alias', 0 FROM sites WHERE name = 'checks';",
            )
            .unwrap();
            for statement in [
                "INSERT INTO files (site_id, path, kind, hash, size)
                 SELECT id, 'bad-file', 1, 'check-hash', 0 FROM sites WHERE name = 'checks'",
                "INSERT INTO allocated_entries
                    (site_id, path, kind, hash, size, naming_mode, prefix, suffix, media_type)
                 SELECT id, 'bad-allocated', 0, 'check-hash', 0, 0, '', '', ''
                 FROM sites WHERE name = 'checks'",
                "INSERT INTO aliases (site_id, path, kind, canonical_target)
                 SELECT id, 'bad-alias', 0, 'target' FROM sites WHERE name = 'checks'",
            ] {
                assert!(
                    db.batch_execute(statement).is_err(),
                    "schema upgraded from {version} accepted {statement}"
                );
            }
        }
    }

    #[test]
    fn unsupported_versions_are_rejected_without_changes() {
        let (_root, mut db) = connection();
        db.batch_execute("PRAGMA user_version = 99").unwrap();
        assert!(matches!(
            migrate(&mut db),
            Err(MigrationError::UnsupportedVersion(99))
        ));
        assert_eq!(schema_version(&mut db).unwrap(), 99);
    }

    #[test]
    fn migration_checksum_drift_is_rejected() {
        let (_root, mut db) = connection();
        migrate(&mut db).unwrap();
        let value = metadata::table
            .find("schema.migration.v11")
            .select(metadata::value)
            .first::<String>(&mut db)
            .unwrap();
        let mut record: MigrationRecord = serde_json::from_str(&value).unwrap();
        record.schema_hash = "wrong".to_string();
        diesel::update(metadata::table.find("schema.migration.v11"))
            .set(metadata::value.eq(serde_json::to_string(&record).unwrap()))
            .execute(&mut db)
            .unwrap();
        assert!(matches!(
            migrate(&mut db),
            Err(MigrationError::Metadata(message))
                if message == "schema checksum drift for version 11"
        ));
    }

    #[test]
    fn migration_records_hash_the_executed_program() {
        let (_fresh_root, mut fresh) = connection();
        migrate(&mut fresh).unwrap();
        let fresh_value = metadata::table
            .find("schema.migration.v11")
            .select(metadata::value)
            .first::<String>(&mut fresh)
            .unwrap();
        let fresh_record: MigrationRecord = serde_json::from_str(&fresh_value).unwrap();
        assert_eq!(fresh_record.program, MigrationProgram::FreshV11);

        let (_upgrade_root, mut upgrade) = connection();
        migrate(&mut upgrade).unwrap();
        downgrade_to_v2(&mut upgrade).unwrap();
        migrate(&mut upgrade).unwrap();
        let upgrade_value = metadata::table
            .find("schema.migration.v11")
            .select(metadata::value)
            .first::<String>(&mut upgrade)
            .unwrap();
        let upgrade_record: MigrationRecord = serde_json::from_str(&upgrade_value).unwrap();
        assert_eq!(upgrade_record.program, MigrationProgram::V2ToV11);
        assert_ne!(fresh_record.program_hash, upgrade_record.program_hash);
    }

    #[test]
    fn v7_and_v8_program_hashes_include_user_version_transition() {
        for (program, baseline, statements) in [
            (
                MigrationProgram::V7ToV11,
                "baseline-v7",
                schema::upgrade_v7_to_v8()
                    .into_iter()
                    .chain(schema::upgrade_v8_to_v9())
                    .chain(schema::upgrade_v9_to_v10())
                    .chain(schema::upgrade_v10_to_v11())
                    .collect::<Vec<_>>(),
            ),
            (
                MigrationProgram::V8ToV11,
                "baseline-v8",
                schema::upgrade_v8_to_v9()
                    .into_iter()
                    .chain(schema::upgrade_v9_to_v10())
                    .chain(schema::upgrade_v10_to_v11())
                    .collect::<Vec<_>>(),
            ),
        ] {
            let mut exact = baseline.to_string();
            for statement in &statements {
                write!(exact, ";\n{statement}").unwrap();
            }
            let without_transition = blake3::hash(exact.as_bytes()).to_hex().to_string();
            write!(
                exact,
                ";\nPRAGMA user_version = {}",
                schema::LATEST_SCHEMA_VERSION
            )
            .unwrap();
            let with_transition = blake3::hash(exact.as_bytes()).to_hex().to_string();

            assert_eq!(migration_program_hash(program), with_transition);
            assert_ne!(migration_program_hash(program), without_transition);
        }
    }
}

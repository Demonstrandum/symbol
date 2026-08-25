use std::fmt::Write as _;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::BigInt;
use diesel::sqlite::SqliteConnection;
use serde::{Deserialize, Serialize};

use super::schema;
use crate::schema::metadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub upgraded_from_v2: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("unsupported database schema version {0}")]
    UnsupportedVersion(i64),
    #[error(transparent)]
    Database(#[from] diesel::result::Error),
    #[error("invalid migration metadata: {0}")]
    Metadata(String),
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
}

pub fn migrate(db: &mut SqliteConnection) -> Result<MigrationOutcome, MigrationError> {
    let version = schema_version(db)?;
    let (outcome, program) = match version {
        0 => {
            db.transaction::<_, MigrationError, _>(|connection| {
                execute(connection, &schema::schema_sql())
            })?;
            (
                MigrationOutcome {
                    upgraded_from_v2: false,
                },
                MigrationProgram::FreshV6,
            )
        }
        2 => {
            db.transaction::<_, MigrationError, _>(|connection| {
                for statement in schema::upgrade_v2_to_v6() {
                    execute(connection, &statement)?;
                }
                set_schema_version(connection, schema::LATEST_SCHEMA_VERSION)
            })?;
            (
                MigrationOutcome {
                    upgraded_from_v2: true,
                },
                MigrationProgram::V2ToV6,
            )
        }
        schema::LATEST_SCHEMA_VERSION => (
            MigrationOutcome {
                upgraded_from_v2: false,
            },
            MigrationProgram::BaselineV6,
        ),
        unsupported => return Err(MigrationError::UnsupportedVersion(unsupported)),
    };
    ensure_migration_record(db, program)?;
    Ok(outcome)
}

pub fn schema_version(db: &mut SqliteConnection) -> Result<i64, diesel::result::Error> {
    diesel::sql_query("PRAGMA user_version")
        .get_result::<SchemaVersion>(db)
        .map(|version| version.user_version)
}

#[cfg(test)]
pub fn downgrade_to_v2(db: &mut SqliteConnection) -> Result<(), MigrationError> {
    db.transaction::<_, MigrationError, _>(|connection| {
        diesel::delete(metadata::table.find("schema.migration.v6")).execute(connection)?;
        for statement in schema::downgrade_v6_to_v2() {
            execute(connection, &statement)?;
        }
        set_schema_version(connection, 2)
    })
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
    const KEY: &str = "schema.migration.v6";
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

fn migration_program_hash(program: MigrationProgram) -> String {
    let source = match program {
        MigrationProgram::FreshV6 => schema::schema_sql(),
        MigrationProgram::V2ToV6 => {
            let mut source = schema::upgrade_v2_to_v6().join(";\n");
            write!(
                source,
                ";\nPRAGMA user_version = {}",
                schema::LATEST_SCHEMA_VERSION
            )
            .expect("writing to String cannot fail");
            source
        }
        MigrationProgram::BaselineV6 => {
            format!("baseline-v6\n{}", schema::schema_sql())
        }
    };
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use diesel::Connection;
    use diesel::sql_types::Text;

    use super::*;

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

    fn connection() -> (tempfile::TempDir, SqliteConnection) {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("symbol.db");
        let connection = SqliteConnection::establish(&database.to_string_lossy()).unwrap();
        (root, connection)
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
            ("index", "expiry_policies_deadline", "expiry_policies"),
            ("index", "expiry_policies_site_kind", "expiry_policies"),
            ("index", "files_hash", "files"),
            ("index", "files_site_prefix", "files"),
            ("index", "idempotency_records_expiry", "idempotency_records"),
            ("index", "management_audit_site", "management_audit"),
            (
                "index",
                "management_idempotency_expiry",
                "management_idempotency",
            ),
            ("index", "undo_files_hash", "undo_files"),
            ("index", "undo_names_stack", "undo_names"),
            ("index", "undo_operations_retention", "undo_operations"),
            ("table", "blobs", "blobs"),
            ("table", "expiry_policies", "expiry_policies"),
            ("table", "files", "files"),
            ("table", "idempotency_records", "idempotency_records"),
            ("table", "management_audit", "management_audit"),
            ("table", "management_idempotency", "management_idempotency"),
            ("table", "management_tombstones", "management_tombstones"),
            ("table", "metadata", "metadata"),
            ("table", "path_aggregates", "path_aggregates"),
            ("table", "sites", "sites"),
            ("table", "undo_expiry_policies", "undo_expiry_policies"),
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
            .find("schema.migration.v6")
            .select(metadata::value)
            .first::<String>(&mut db)
            .unwrap();
        let mut record: MigrationRecord = serde_json::from_str(&value).unwrap();
        record.schema_hash = "wrong".to_string();
        diesel::update(metadata::table.find("schema.migration.v6"))
            .set(metadata::value.eq(serde_json::to_string(&record).unwrap()))
            .execute(&mut db)
            .unwrap();
        assert!(matches!(
            migrate(&mut db),
            Err(MigrationError::Metadata(message))
                if message == "schema checksum drift for version 6"
        ));
    }

    #[test]
    fn migration_records_hash_the_executed_program() {
        let (_fresh_root, mut fresh) = connection();
        migrate(&mut fresh).unwrap();
        let fresh_value = metadata::table
            .find("schema.migration.v6")
            .select(metadata::value)
            .first::<String>(&mut fresh)
            .unwrap();
        let fresh_record: MigrationRecord = serde_json::from_str(&fresh_value).unwrap();
        assert_eq!(fresh_record.program, MigrationProgram::FreshV6);

        let (_upgrade_root, mut upgrade) = connection();
        migrate(&mut upgrade).unwrap();
        downgrade_to_v2(&mut upgrade).unwrap();
        migrate(&mut upgrade).unwrap();
        let upgrade_value = metadata::table
            .find("schema.migration.v6")
            .select(metadata::value)
            .first::<String>(&mut upgrade)
            .unwrap();
        let upgrade_record: MigrationRecord = serde_json::from_str(&upgrade_value).unwrap();
        assert_eq!(upgrade_record.program, MigrationProgram::V2ToV6);
        assert_ne!(fresh_record.program_hash, upgrade_record.program_hash);
    }
}

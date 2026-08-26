use std::fmt;

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use diesel::sqlite::SqliteConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaCatalog {
    tables: Vec<TableCatalog>,
    other_objects: Vec<OtherObject>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableCatalog {
    name: String,
    table_type: String,
    declared_column_count: i64,
    without_rowid: bool,
    strict: bool,
    columns: Vec<ColumnCatalog>,
    foreign_keys: Vec<ForeignKeyCatalog>,
    indexes: Vec<IndexCatalog>,
    check_constraints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnCatalog {
    position: i64,
    name: String,
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
    hidden: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKeyCatalog {
    referenced_table: String,
    on_update: String,
    on_delete: String,
    match_clause: String,
    columns: Vec<ForeignKeyColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKeyColumn {
    position: i64,
    from: String,
    to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IndexCatalog {
    explicit_name: Option<String>,
    unique: bool,
    origin: IndexOrigin,
    partial: bool,
    columns: Vec<IndexColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum IndexOrigin {
    Created,
    UniqueConstraint,
    PrimaryKey,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IndexColumn {
    position: i64,
    column_id: i64,
    name: Option<String>,
    descending: bool,
    collation: Option<String>,
    key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OtherObject {
    object_type: String,
    name: String,
    table_name: String,
    sql: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogDimension {
    Tables,
    TableOptions,
    Columns,
    ForeignKeys,
    Indexes,
    CheckConstraints,
    OtherObjects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDifference {
    table: Option<String>,
    dimension: CatalogDimension,
}

impl fmt::Display for CatalogDifference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dimension = match self.dimension {
            CatalogDimension::Tables => "tables",
            CatalogDimension::TableOptions => "table options",
            CatalogDimension::Columns => "columns/types/null/defaults/primary keys",
            CatalogDimension::ForeignKeys => "foreign keys",
            CatalogDimension::Indexes => "indexes/unique constraints",
            CatalogDimension::CheckConstraints => "check constraints",
            CatalogDimension::OtherObjects => "views/triggers",
        };
        if let Some(table) = &self.table {
            write!(formatter, "{dimension} differ for table {table}")
        } else {
            write!(formatter, "{dimension} differ")
        }
    }
}

impl std::error::Error for CatalogDifference {}

impl SchemaCatalog {
    pub fn load(db: &mut SqliteConnection) -> QueryResult<Self> {
        let table_rows = diesel::sql_query(
            "SELECT tables.name,
                    tables.type AS table_type,
                    tables.ncol AS declared_column_count,
                    tables.wr AS without_rowid,
                    tables.strict,
                    schema.sql
             FROM pragma_table_list AS tables
             JOIN sqlite_schema AS schema
               ON schema.name = tables.name AND schema.type = 'table'
             WHERE tables.schema = 'main'
               AND tables.name NOT LIKE 'sqlite_%'
             ORDER BY tables.name",
        )
        .load::<TableRow>(db)?;
        let mut tables = Vec::with_capacity(table_rows.len());
        for row in table_rows {
            let name = normalize_identifier(&row.name);
            let columns = load_columns(db, &row.name)?;
            let foreign_keys = load_foreign_keys(db, &row.name)?;
            let indexes = load_indexes(db, &row.name)?;
            let check_constraints = row
                .sql
                .as_deref()
                .map_or_else(Vec::new, extract_check_constraints);
            tables.push(TableCatalog {
                name,
                table_type: normalize_identifier(&row.table_type),
                declared_column_count: row.declared_column_count,
                without_rowid: row.without_rowid != 0,
                strict: row.strict != 0,
                columns,
                foreign_keys,
                indexes,
                check_constraints,
            });
        }

        let mut other_objects = diesel::sql_query(
            "SELECT type AS object_type, name, tbl_name AS table_name, sql
             FROM sqlite_schema
             WHERE type IN ('view', 'trigger')
             ORDER BY type, name",
        )
        .load::<OtherObjectRow>(db)?
        .into_iter()
        .map(|row| OtherObject {
            object_type: normalize_identifier(&row.object_type),
            name: normalize_identifier(&row.name),
            table_name: normalize_identifier(&row.table_name),
            sql: row.sql.as_deref().map(normalize_sql_fragment),
        })
        .collect::<Vec<_>>();
        other_objects.sort();

        Ok(Self {
            tables,
            other_objects,
        })
    }

    pub fn difference(&self, actual: &Self) -> Option<CatalogDifference> {
        if self
            .tables
            .iter()
            .map(|table| &table.name)
            .ne(actual.tables.iter().map(|table| &table.name))
        {
            return Some(CatalogDifference {
                table: None,
                dimension: CatalogDimension::Tables,
            });
        }
        for (expected, observed) in self.tables.iter().zip(&actual.tables) {
            let table = || Some(expected.name.clone());
            if expected.table_type != observed.table_type
                || expected.declared_column_count != observed.declared_column_count
                || expected.without_rowid != observed.without_rowid
                || expected.strict != observed.strict
            {
                return Some(CatalogDifference {
                    table: table(),
                    dimension: CatalogDimension::TableOptions,
                });
            }
            if expected.columns != observed.columns {
                return Some(CatalogDifference {
                    table: table(),
                    dimension: CatalogDimension::Columns,
                });
            }
            if expected.foreign_keys != observed.foreign_keys {
                return Some(CatalogDifference {
                    table: table(),
                    dimension: CatalogDimension::ForeignKeys,
                });
            }
            if expected.indexes != observed.indexes {
                return Some(CatalogDifference {
                    table: table(),
                    dimension: CatalogDimension::Indexes,
                });
            }
            if expected.check_constraints != observed.check_constraints {
                return Some(CatalogDifference {
                    table: table(),
                    dimension: CatalogDimension::CheckConstraints,
                });
            }
        }
        (self.other_objects != actual.other_objects).then_some(CatalogDifference {
            table: None,
            dimension: CatalogDimension::OtherObjects,
        })
    }
}

fn load_columns(db: &mut SqliteConnection, table: &str) -> QueryResult<Vec<ColumnCatalog>> {
    diesel::sql_query(
        "SELECT cid AS position,
                name,
                type AS declared_type,
                \"notnull\" AS not_null,
                dflt_value AS default_value,
                pk AS primary_key_position,
                hidden
         FROM pragma_table_xinfo(?)
         ORDER BY cid",
    )
    .bind::<Text, _>(table)
    .load::<ColumnRow>(db)
    .map(|rows| {
        rows.into_iter()
            .map(|row| ColumnCatalog {
                position: row.position,
                name: normalize_identifier(&row.name),
                declared_type: normalize_type(&row.declared_type),
                not_null: row.not_null != 0,
                default_value: row.default_value.as_deref().map(normalize_sql_fragment),
                primary_key_position: row.primary_key_position,
                hidden: row.hidden,
            })
            .collect()
    })
}

fn load_foreign_keys(
    db: &mut SqliteConnection,
    table: &str,
) -> QueryResult<Vec<ForeignKeyCatalog>> {
    let rows = diesel::sql_query(
        "SELECT id,
                seq AS position,
                \"table\" AS referenced_table,
                \"from\" AS source_column,
                \"to\" AS target_column,
                on_update,
                on_delete,
                \"match\" AS match_clause
         FROM pragma_foreign_key_list(?)
         ORDER BY id, seq",
    )
    .bind::<Text, _>(table)
    .load::<ForeignKeyRow>(db)?;

    let mut grouped = Vec::<(i64, ForeignKeyCatalog)>::new();
    for row in rows {
        let column = ForeignKeyColumn {
            position: row.position,
            from: normalize_identifier(&row.source_column),
            to: row.target_column.as_deref().map(normalize_identifier),
        };
        if let Some((id, foreign_key)) = grouped.last_mut()
            && *id == row.id
        {
            foreign_key.columns.push(column);
            continue;
        }
        grouped.push((
            row.id,
            ForeignKeyCatalog {
                referenced_table: normalize_identifier(&row.referenced_table),
                on_update: normalize_identifier(&row.on_update),
                on_delete: normalize_identifier(&row.on_delete),
                match_clause: normalize_identifier(&row.match_clause),
                columns: vec![column],
            },
        ));
    }
    let mut foreign_keys = grouped
        .into_iter()
        .map(|(_, foreign_key)| foreign_key)
        .collect::<Vec<_>>();
    foreign_keys.sort();
    Ok(foreign_keys)
}

fn load_indexes(db: &mut SqliteConnection, table: &str) -> QueryResult<Vec<IndexCatalog>> {
    let rows = diesel::sql_query(
        "SELECT name,
                \"unique\" AS is_unique,
                origin,
                partial
         FROM pragma_index_list(?)",
    )
    .bind::<Text, _>(table)
    .load::<IndexRow>(db)?;
    let mut indexes = Vec::with_capacity(rows.len());
    for row in rows {
        let origin = match row.origin.as_str() {
            "c" => IndexOrigin::Created,
            "u" => IndexOrigin::UniqueConstraint,
            "pk" => IndexOrigin::PrimaryKey,
            other => IndexOrigin::Other(normalize_identifier(other)),
        };
        let explicit_name =
            matches!(origin, IndexOrigin::Created).then(|| normalize_identifier(&row.name));
        let columns = diesel::sql_query(
            "SELECT seqno AS position,
                    cid AS column_id,
                    name,
                    \"desc\" AS descending,
                    coll AS collation,
                    \"key\" AS is_key
             FROM pragma_index_xinfo(?)
             ORDER BY seqno",
        )
        .bind::<Text, _>(&row.name)
        .load::<IndexColumnRow>(db)?
        .into_iter()
        .map(|column| IndexColumn {
            position: column.position,
            column_id: column.column_id,
            name: column.name.as_deref().map(normalize_identifier),
            descending: column.descending != 0,
            collation: column.collation.as_deref().map(normalize_identifier),
            key: column.is_key != 0,
        })
        .collect();
        indexes.push(IndexCatalog {
            explicit_name,
            unique: row.is_unique != 0,
            origin,
            partial: row.partial != 0,
            columns,
        });
    }
    indexes.sort();
    Ok(indexes)
}

fn normalize_identifier(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn normalize_type(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn normalize_sql_fragment(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut quote = None;
    let mut whitespace_pending = false;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if let Some(delimiter) = quote {
            normalized.push(character);
            if character == delimiter {
                if characters.peek() == Some(&delimiter) {
                    normalized.push(characters.next().expect("peeked quote"));
                } else {
                    quote = None;
                }
            }
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            if whitespace_pending && !normalized.is_empty() {
                normalized.push(' ');
            }
            whitespace_pending = false;
            normalized.push(character);
            quote = Some(character);
        } else if character.is_whitespace() {
            whitespace_pending = true;
        } else {
            if whitespace_pending && !normalized.is_empty() {
                normalized.push(' ');
            }
            whitespace_pending = false;
            normalized.extend(character.to_lowercase());
        }
    }
    normalized.trim().to_string()
}

fn extract_check_constraints(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut constraints = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if matches!(bytes[position], b'\'' | b'"' | b'`') {
            position = skip_quoted(bytes, position, bytes[position]);
            continue;
        }
        if bytes[position] == b'[' {
            position += 1;
            while position < bytes.len() && bytes[position] != b']' {
                position += 1;
            }
            position += usize::from(position < bytes.len());
            continue;
        }
        if is_identifier_byte(bytes[position]) {
            let start = position;
            while position < bytes.len() && is_identifier_byte(bytes[position]) {
                position += 1;
            }
            if !sql[start..position].eq_ignore_ascii_case("check") {
                continue;
            }
            while position < bytes.len() && bytes[position].is_ascii_whitespace() {
                position += 1;
            }
            if position >= bytes.len() || bytes[position] != b'(' {
                continue;
            }
            let expression_start = position + 1;
            position += 1;
            let mut depth = 1;
            while position < bytes.len() && depth > 0 {
                match bytes[position] {
                    delimiter @ (b'\'' | b'"' | b'`') => {
                        position = skip_quoted(bytes, position, delimiter);
                    }
                    b'[' => {
                        position += 1;
                        while position < bytes.len() && bytes[position] != b']' {
                            position += 1;
                        }
                        position += usize::from(position < bytes.len());
                    }
                    b'(' => {
                        depth += 1;
                        position += 1;
                    }
                    b')' => {
                        depth -= 1;
                        position += 1;
                    }
                    _ => position += 1,
                }
            }
            if depth == 0 {
                constraints.push(normalize_sql_fragment(&sql[expression_start..position - 1]));
            }
            continue;
        }
        position += 1;
    }
    constraints.sort();
    constraints
}

const fn skip_quoted(bytes: &[u8], mut position: usize, delimiter: u8) -> usize {
    position += 1;
    while position < bytes.len() {
        if bytes[position] == delimiter {
            position += 1;
            if position < bytes.len() && bytes[position] == delimiter {
                position += 1;
                continue;
            }
            break;
        }
        position += 1;
    }
    position
}

const fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[derive(QueryableByName)]
struct TableRow {
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    table_type: String,
    #[diesel(sql_type = BigInt)]
    declared_column_count: i64,
    #[diesel(sql_type = BigInt)]
    without_rowid: i64,
    #[diesel(sql_type = BigInt)]
    strict: i64,
    #[diesel(sql_type = Nullable<Text>)]
    sql: Option<String>,
}

#[derive(QueryableByName)]
struct ColumnRow {
    #[diesel(sql_type = BigInt)]
    position: i64,
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    declared_type: String,
    #[diesel(sql_type = BigInt)]
    not_null: i64,
    #[diesel(sql_type = Nullable<Text>)]
    default_value: Option<String>,
    #[diesel(sql_type = BigInt)]
    primary_key_position: i64,
    #[diesel(sql_type = BigInt)]
    hidden: i64,
}

#[derive(QueryableByName)]
struct ForeignKeyRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = BigInt)]
    position: i64,
    #[diesel(sql_type = Text)]
    referenced_table: String,
    #[diesel(sql_type = Text)]
    source_column: String,
    #[diesel(sql_type = Nullable<Text>)]
    target_column: Option<String>,
    #[diesel(sql_type = Text)]
    on_update: String,
    #[diesel(sql_type = Text)]
    on_delete: String,
    #[diesel(sql_type = Text)]
    match_clause: String,
}

#[derive(QueryableByName)]
struct IndexRow {
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = BigInt)]
    is_unique: i64,
    #[diesel(sql_type = Text)]
    origin: String,
    #[diesel(sql_type = BigInt)]
    partial: i64,
}

#[derive(QueryableByName)]
struct IndexColumnRow {
    #[diesel(sql_type = BigInt)]
    position: i64,
    #[diesel(sql_type = BigInt)]
    column_id: i64,
    #[diesel(sql_type = Nullable<Text>)]
    name: Option<String>,
    #[diesel(sql_type = BigInt)]
    descending: i64,
    #[diesel(sql_type = Nullable<Text>)]
    collation: Option<String>,
    #[diesel(sql_type = BigInt)]
    is_key: i64,
}

#[derive(QueryableByName)]
struct OtherObjectRow {
    #[diesel(sql_type = Text)]
    object_type: String,
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    table_name: String,
    #[diesel(sql_type = Nullable<Text>)]
    sql: Option<String>,
}

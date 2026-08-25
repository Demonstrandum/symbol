#[allow(dead_code)]
#[path = "../src/database/schema.rs"]
mod schema;

fn main() {
    print!("{}", schema::schema_sql());
}

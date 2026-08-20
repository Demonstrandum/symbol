use std::fs;
use std::path::Path;

use lightningcss::stylesheet::{ParserOptions, StyleSheet};

const STYLESHEETS: &[&str] = &["base.css", "browse.css", "docs.css"];

fn main() {
    for name in STYLESHEETS {
        let path = Path::new("static").join(name);
        println!("cargo::rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        StyleSheet::parse(
            &source,
            ParserOptions {
                filename: path.display().to_string(),
                ..ParserOptions::default()
            },
        )
        .unwrap_or_else(|error| panic!("invalid CSS in {}: {error:?}", path.display()));
    }
}

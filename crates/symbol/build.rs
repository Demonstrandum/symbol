use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use generation::{
    BUMP_EXPECTED_ENV, BUMP_INTENT_ENV, BUMP_TARGET_ENV, BuildProvenance, BumpIntent, COMMIT_ENV,
    DIRTY_ENV, GENERATION_MODE_ENV, GenerationMetadata, GenerationMode, VersionLedger,
    bump_intent_from_environment, canonical_input_directories, canonical_input_paths,
    generate_artifacts, generation_mode_from_environment, git_watch_paths, prepare_ledger,
    prepare_ledger_with_intent, preview_ledger, reconcile_snapshot, tracked_git_files,
    write_generated_file,
};
use lightningcss::stylesheet::{ParserOptions, StyleSheet};

#[allow(dead_code)]
#[path = "src/database/schema.rs"]
mod database_schema;
#[allow(dead_code, unused_imports)]
#[path = "generation/mod.rs"]
mod generation;

const STYLESHEETS: &[&str] = &["base.css", "browse.css", "docs.css"];

fn main() {
    run().unwrap_or_else(|error| panic!("deterministic generation failed: {error}"));
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!symbol_contract::ENDPOINTS.is_empty());
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let root = manifest_dir
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));
    emit_rerun_inputs(&root)?;

    let mode = generation_mode_from_environment()?;
    let bump_intent = bump_intent_from_environment()?;
    match mode {
        GenerationMode::Update => generate_writable(&root, &out_dir, bump_intent)?,
        GenerationMode::ReadOnly if bump_intent == BumpIntent::Automatic => {
            generate_read_only(&root, &out_dir)?;
        }
        GenerationMode::ReadOnly => {
            prepare_ledger_with_intent(&root, mode, bump_intent)?;
        }
    }

    for name in STYLESHEETS {
        let path = root.join("static").join(name);
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
    Ok(())
}

fn generate_writable(
    root: &Path,
    out_dir: &Path,
    bump_intent: BumpIntent,
) -> Result<(), Box<dyn std::error::Error>> {
    let ledger = prepare_ledger_with_intent(root, GenerationMode::Update, bump_intent)?;
    let schema = database_schema::schema_sql();
    reconcile_snapshot(
        root,
        &root.join("schema.sql"),
        &schema,
        GenerationMode::Update,
    )?;
    write_outputs(root, out_dir, ledger, &schema)
}

fn generate_read_only(root: &Path, out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let ledger = preview_ledger(root)?;
    let schema = database_schema::schema_sql();
    write_outputs(root, out_dir, ledger, &schema)?;

    let ledger_fresh = prepare_ledger(root, GenerationMode::ReadOnly);
    let schema_fresh = reconcile_snapshot(
        root,
        &root.join("schema.sql"),
        &schema,
        GenerationMode::ReadOnly,
    );
    ledger_fresh?;
    schema_fresh?;
    Ok(())
}

fn write_outputs(
    root: &Path,
    out_dir: &Path,
    ledger: VersionLedger,
    schema: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let provenance = BuildProvenance::discover(root)?;
    println!("cargo::rustc-env=SYMBOL_API_VERSION={}", ledger.version);
    println!(
        "cargo::rustc-env=SYMBOL_API_REVISION={}",
        ledger.absolute_revision
    );
    println!(
        "cargo::rustc-env=SYMBOL_API_SOURCE_HASH={}",
        ledger.source_hash
    );
    println!("cargo::rustc-env=SYMBOL_API_COMMIT={}", provenance.commit);
    println!(
        "cargo::rustc-env=SYMBOL_API_DIRTY={}",
        if provenance.dirty { "true" } else { "false" }
    );
    write_generated_file(&out_dir.join("schema.sql"), schema)?;
    let metadata = GenerationMetadata { ledger, provenance };
    generate_artifacts(root, &metadata)?.write_to(out_dir)?;
    let api_source = fs::read_to_string(root.join("API.md"))?;
    for page in generation::docs::compile(&api_source)? {
        write_generated_file(
            &out_dir.join(format!("api-doc-{}.md", page.manual.slug())),
            &page.markdown,
        )?;
        write_generated_file(
            &out_dir.join(format!("api-doc-{}.html", page.manual.slug())),
            &page.html,
        )?;
    }
    Ok(())
}

fn emit_rerun_inputs(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-env-changed={GENERATION_MODE_ENV}");
    println!("cargo::rerun-if-env-changed={BUMP_INTENT_ENV}");
    println!("cargo::rerun-if-env-changed={BUMP_EXPECTED_ENV}");
    println!("cargo::rerun-if-env-changed={BUMP_TARGET_ENV}");
    println!("cargo::rerun-if-env-changed={COMMIT_ENV}");
    println!("cargo::rerun-if-env-changed={DIRTY_ENV}");
    println!(
        "cargo::rerun-if-changed={}",
        root.join("api-version.toml").display()
    );
    println!(
        "cargo::rerun-if-changed={}",
        root.join("schema.sql").display()
    );
    println!(
        "cargo::rerun-if-changed={}",
        root.join("crates/symbol/src/database/schema.rs").display()
    );
    for path in canonical_input_paths(root)? {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    for path in canonical_input_directories(root) {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    if env::var_os(COMMIT_ENV).is_none() && env::var_os(DIRTY_ENV).is_none() {
        for path in git_watch_paths(root)? {
            println!("cargo::rerun-if-changed={}", path.display());
        }
        for path in tracked_git_files(root)? {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    Ok(())
}

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{
    ApiVersion, BumpIntent, BumpKind, ExplicitBumpRequest, GenerationError, GenerationMode,
    bump_intent_from_environment, canonical_input_paths, canonical_source_hash, prepare_ledger,
    prepare_ledger_with_intent, preview_ledger, read_ledger, reconcile_snapshot,
};
use tempfile::TempDir;

const WORKER_ROOT_ENV: &str = "SYMBOL_GENERATION_TEST_ROOT";
const WRAPPER_WORKER_ROOT_ENV: &str = "SYMBOL_GENERATION_WRAPPER_TEST_ROOT";
const WRAPPER_BARRIER_ENV: &str = "SYMBOL_GENERATION_WRAPPER_TEST_BARRIER";
const WRAPPER_COUNT_ENV: &str = "SYMBOL_GENERATION_WRAPPER_TEST_COUNT";

#[test]
fn unchanged_changed_and_interrupted_workspaces_update_once() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    assert_eq!(initial.version, ApiVersion::new(0, 1, 0));
    assert_eq!(initial.absolute_revision, 1);
    assert_eq!(
        prepare_ledger(root, GenerationMode::Update).unwrap(),
        initial
    );

    append(&root.join("API.md"), "\nphase-six-test-change\n");
    let preview = preview_ledger(root).unwrap();
    assert_eq!(preview.version, ApiVersion::new(0, 1, 1));
    assert_eq!(preview.absolute_revision, 2);
    assert_eq!(read_ledger(root).unwrap(), initial);
    let stale = prepare_ledger(root, GenerationMode::ReadOnly).unwrap_err();
    assert!(matches!(stale, GenerationError::StaleLedger { .. }));

    fs::write(
        root.join(".api-version.toml.tmp.interrupted"),
        b"version = \"broken",
    )
    .unwrap();
    assert_eq!(read_ledger(root).unwrap(), initial);

    let updated = prepare_ledger(root, GenerationMode::Update).unwrap();
    assert_eq!(updated.version, ApiVersion::new(0, 1, 1));
    assert_eq!(updated.absolute_revision, 2);
    assert_eq!(updated.source_hash, canonical_source_hash(root).unwrap());
    assert_eq!(
        prepare_ledger(root, GenerationMode::Update).unwrap(),
        updated
    );

    fs::write(root.join("not-canonical.txt"), b"ignored\n").unwrap();
    assert_eq!(
        prepare_ledger(root, GenerationMode::Update).unwrap(),
        updated
    );
}

#[test]
fn parallel_processes_increment_patch_and_revision_once() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    append(&root.join("API.md"), "\nparallel-change\n");

    let executable = std::env::current_exe().unwrap();
    let mut children = Vec::new();
    for _ in 0..12 {
        children.push(
            Command::new(&executable)
                .arg("ledger_process_worker")
                .env(WORKER_ROOT_ENV, root)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let final_ledger = read_ledger(root).unwrap();
    assert_eq!(final_ledger.version, initial.version.next_patch());
    assert_eq!(
        final_ledger.absolute_revision,
        initial.absolute_revision + 1
    );
    assert_eq!(
        final_ledger.source_hash,
        canonical_source_hash(root).unwrap()
    );
}

#[test]
fn ledger_process_worker() {
    let Some(root) = std::env::var_os(WORKER_ROOT_ENV) else {
        return;
    };
    prepare_ledger(Path::new(&root), GenerationMode::Update).unwrap();
}

#[test]
fn checked_snapshots_update_locally_and_fail_stale_read_only() {
    let workspace = tempfile::tempdir().unwrap();
    let snapshot = workspace.path().join("schema.sql");
    reconcile_snapshot(
        workspace.path(),
        &snapshot,
        "schema one\n",
        GenerationMode::Update,
    )
    .unwrap();
    reconcile_snapshot(
        workspace.path(),
        &snapshot,
        "schema one\n",
        GenerationMode::ReadOnly,
    )
    .unwrap();

    let stale = reconcile_snapshot(
        workspace.path(),
        &snapshot,
        "schema two\n",
        GenerationMode::ReadOnly,
    )
    .unwrap_err();
    assert!(matches!(stale, GenerationError::StaleSnapshot { .. }));
    assert_eq!(fs::read_to_string(&snapshot).unwrap(), "schema one\n");
}

#[test]
fn dirty_source_minor_wrapper_intent_is_one_atomic_revision() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    append(&root.join("API.md"), "\nminor-contract-change\n");

    let bumped = prepare_ledger_with_intent(
        root,
        GenerationMode::Update,
        explicit_request(&initial, BumpKind::Minor),
    )
    .unwrap();
    assert_eq!(bumped.version, initial.version.next_minor());
    assert_eq!(bumped.absolute_revision, initial.absolute_revision + 1);
    assert_eq!(bumped.source_hash, canonical_source_hash(root).unwrap());
}

#[test]
fn dirty_source_major_wrapper_intent_is_one_atomic_revision() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    append(&root.join("API.md"), "\nmajor-contract-change\n");

    let bumped = prepare_ledger_with_intent(
        root,
        GenerationMode::Update,
        explicit_request(&initial, BumpKind::Major),
    )
    .unwrap();
    assert_eq!(bumped.version, initial.version.next_major());
    assert_eq!(bumped.absolute_revision, initial.absolute_revision + 1);
    assert_eq!(bumped.source_hash, canonical_source_hash(root).unwrap());
}

#[test]
fn later_same_hash_semantic_bumps_each_consume_a_revision() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    append(&root.join("API.md"), "\nreclassified-contract-change\n");

    let automatic = prepare_ledger(root, GenerationMode::Update).unwrap();
    assert_eq!(automatic.version, initial.version.next_patch());
    assert_eq!(automatic.absolute_revision, initial.absolute_revision + 1);

    let minor_request = explicit_request(&automatic, BumpKind::Minor);
    let minor =
        prepare_ledger_with_intent(root, GenerationMode::Update, minor_request.clone()).unwrap();
    assert_eq!(minor.version, automatic.version.next_minor());
    assert_eq!(minor.absolute_revision, automatic.absolute_revision + 1);
    assert_eq!(minor.source_hash, automatic.source_hash);

    assert_eq!(
        prepare_ledger_with_intent(root, GenerationMode::Update, minor_request).unwrap(),
        minor
    );

    let major = prepare_ledger_with_intent(
        root,
        GenerationMode::Update,
        explicit_request(&minor, BumpKind::Major),
    )
    .unwrap();
    assert_eq!(major.version, minor.version.next_major());
    assert_eq!(major.absolute_revision, minor.absolute_revision + 1);
    assert_eq!(major.source_hash, automatic.source_hash);
}

#[test]
fn unrelated_ledger_movement_conflicts_with_explicit_request() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    let request = explicit_request(&initial, BumpKind::Minor);
    append(&root.join("API.md"), "\nunrelated-movement\n");
    prepare_ledger(root, GenerationMode::Update).unwrap();

    let error = prepare_ledger_with_intent(root, GenerationMode::Update, request).unwrap_err();
    assert!(matches!(error, GenerationError::BumpConflict { .. }));
}

#[test]
fn parallel_minor_wrappers_converge_to_one_bump() {
    parallel_wrappers_converge("bump-minor", BumpKind::Minor);
}

#[test]
fn parallel_major_wrappers_converge_to_one_bump() {
    parallel_wrappers_converge("bump-major", BumpKind::Major);
}

#[test]
fn wrapper_bump_worker() {
    let Some(root) = std::env::var_os(WRAPPER_WORKER_ROOT_ENV) else {
        return;
    };
    let barrier = std::env::var_os(WRAPPER_BARRIER_ENV).unwrap();
    let count = std::env::var(WRAPPER_COUNT_ENV)
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let barrier = Path::new(&barrier);
    fs::write(barrier.join(std::process::id().to_string()), b"ready").unwrap();
    for _ in 0..5_000 {
        if fs::read_dir(barrier).unwrap().count() >= count {
            let intent = bump_intent_from_environment().unwrap();
            prepare_ledger_with_intent(Path::new(&root), GenerationMode::Update, intent).unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("parallel wrapper barrier timed out");
}

#[test]
fn canonical_hash_ignores_unrelated_sources_and_lock_packages() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = canonical_source_hash(root).unwrap();

    let unrelated_source = root.join("crates/symbol/src/expiry.rs");
    fs::create_dir_all(unrelated_source.parent().unwrap()).unwrap();
    fs::write(unrelated_source, b"unrelated implementation change\n").unwrap();
    let unrelated_css = root.join("static/docs.css");
    fs::write(unrelated_css, b"unrelated style change\n").unwrap();
    append(
        &root.join("crates/symbol/Cargo.toml"),
        "\n[package.metadata.review-test]\nvalue = \"ignored\"\n",
    );
    append(
        &root.join("Cargo.lock"),
        "\n[[package]]\nname = \"unrelated-review-test\"\nversion = \"1.0.0\"\n",
    );

    assert_eq!(canonical_source_hash(root).unwrap(), initial);
}

#[test]
fn canonical_hash_tracks_generator_source_and_swc_resolution() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = canonical_source_hash(root).unwrap();
    append(
        &root.join("crates/symbol/generation/ledger.rs"),
        "\n// generator behavior change\n",
    );
    assert_ne!(canonical_source_hash(root).unwrap(), initial);

    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = canonical_source_hash(root).unwrap();
    let lock_path = root.join("Cargo.lock");
    let lock = fs::read_to_string(&lock_path).unwrap();
    let changed = lock.replacen(
        "name = \"swc_core\"\nversion = \"77.0.3\"",
        "name = \"swc_core\"\nversion = \"77.0.4\"",
        1,
    );
    assert_ne!(changed, lock, "SWC lock entry must exist");
    fs::write(lock_path, changed).unwrap();
    assert_ne!(canonical_source_hash(root).unwrap(), initial);
}

#[test]
fn canonical_hash_tracks_generator_package_and_build_dependency_configuration() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = canonical_source_hash(root).unwrap();
    let manifest_path = root.join("crates/symbol/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let changed = manifest.replacen("version = \"0.1.0\"", "version = \"0.1.1\"", 1);
    fs::write(&manifest_path, changed).unwrap();
    assert_ne!(canonical_source_hash(root).unwrap(), initial);

    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = canonical_source_hash(root).unwrap();
    let manifest_path = root.join("crates/symbol/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let (prefix, build_dependencies) = manifest.split_once("[build-dependencies]").unwrap();
    let changed_build_dependencies = build_dependencies.replacen(
        "blake3 = \"1\"",
        "blake3 = { version = \"1\", default-features = false, features = [\"std\"] }",
        1,
    );
    fs::write(
        &manifest_path,
        format!("{prefix}[build-dependencies]{changed_build_dependencies}"),
    )
    .unwrap();
    assert_ne!(canonical_source_hash(root).unwrap(), initial);
}

#[test]
fn canonical_hash_ignores_normal_dependencies_lints_css_and_test_sources() {
    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = canonical_source_hash(root).unwrap();
    let manifest_path = root.join("crates/symbol/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let changed = manifest
        .replacen("axum = \"0.8\"", "axum = \"9.9\"", 1)
        .replace("all = { level = \"warn\"", "all = { level = \"allow\"");
    fs::write(manifest_path, changed).unwrap();
    fs::write(root.join("static/review.css"), "body { color: red; }\n").unwrap();
    fs::write(
        root.join("crates/symbol/generation/ledger_tests.rs"),
        "#[test]\nfn unrelated_test_change() {}\n",
    )
    .unwrap();
    let manifest_path = root.join("crates/symbol/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let (prefix, build_dependencies) = manifest.split_once("[build-dependencies]").unwrap();
    let reordered = build_dependencies.replacen(
        "features = [\"common\", \"ecma_ast\", \"ecma_codegen\"",
        "features = [\"ecma_codegen\", \"ecma_ast\", \"common\"",
        1,
    );
    assert_ne!(reordered, build_dependencies);
    fs::write(
        manifest_path,
        format!("{prefix}[build-dependencies]{reordered}"),
    )
    .unwrap();
    assert_eq!(canonical_source_hash(root).unwrap(), initial);
}

fn explicit_request(ledger: &super::VersionLedger, kind: BumpKind) -> BumpIntent {
    let target = match kind {
        BumpKind::Minor => ledger.version.next_minor(),
        BumpKind::Major => ledger.version.next_major(),
    };
    BumpIntent::Explicit(ExplicitBumpRequest {
        kind,
        expected: ledger.clone(),
        target,
    })
}

#[cfg(unix)]
fn parallel_wrappers_converge(command: &str, kind: BumpKind) {
    use std::os::unix::fs::PermissionsExt as _;

    let workspace = copied_workspace();
    let root = workspace.path();
    let initial = prepare_ledger(root, GenerationMode::Update).unwrap();
    append(&root.join("API.md"), "\nparallel-explicit-change\n");

    let harness = tempfile::tempdir().unwrap();
    let fake_bin = harness.path().join("bin");
    let barrier = harness.path().join("barrier");
    fs::create_dir(&fake_bin).unwrap();
    fs::create_dir(&barrier).unwrap();
    let fake_cargo = fake_bin.join("cargo");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nexec \"$SYMBOL_GENERATION_WRAPPER_WORKER\" wrapper_bump_worker\n",
    )
    .unwrap();
    fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(root.join("api-version"), fs::Permissions::from_mode(0o755)).unwrap();

    let executable = std::env::current_exe().unwrap();
    let inherited_path = std::env::var_os("PATH").unwrap();
    let path = std::env::join_paths(
        std::iter::once(fake_bin).chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap();
    let count = 12;
    let mut children = Vec::new();
    for _ in 0..count {
        children.push(
            Command::new(root.join("api-version"))
                .arg(command)
                .env("PATH", &path)
                .env("SYMBOL_GENERATION_WRAPPER_WORKER", &executable)
                .env(WRAPPER_WORKER_ROOT_ENV, root)
                .env(WRAPPER_BARRIER_ENV, &barrier)
                .env(WRAPPER_COUNT_ENV, count.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let final_ledger = read_ledger(root).unwrap();
    let expected_version = match kind {
        BumpKind::Minor => initial.version.next_minor(),
        BumpKind::Major => initial.version.next_major(),
    };
    assert_eq!(final_ledger.version, expected_version);
    assert_eq!(
        final_ledger.absolute_revision,
        initial.absolute_revision + 1
    );
    assert_eq!(
        final_ledger.source_hash,
        canonical_source_hash(root).unwrap()
    );
}

fn copied_workspace() -> TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let destination = tempfile::tempdir().unwrap();
    for source_path in canonical_input_paths(&source).unwrap() {
        let relative = source_path.strip_prefix(&source).unwrap();
        let destination_path = destination.path().join(relative);
        fs::create_dir_all(destination_path.parent().unwrap()).unwrap();
        fs::copy(source_path, destination_path).unwrap();
    }
    destination
}

fn append(path: &Path, contents: &str) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
}

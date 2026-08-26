use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::process::Command;

use super::{
    BuildProvenance, GitQuery, discover_git, git_watch_paths, injected_provenance,
    resolve_with_probe,
};

#[test]
fn clean_and_dirty_git_results_are_distinct() {
    let clean = resolve_with_probe(|query| {
        Ok(match query {
            GitQuery::Head => "abc1230".to_string(),
            GitQuery::Dirty => String::new(),
        })
    })
    .unwrap();
    assert_eq!(
        clean,
        BuildProvenance::from_injected("abc1230".to_string(), false).unwrap()
    );

    let dirty = resolve_with_probe(|query| {
        Ok(match query {
            GitQuery::Head => "abc1230".to_string(),
            GitQuery::Dirty => " M API.md".to_string(),
        })
    })
    .unwrap();
    assert!(dirty.dirty);
}

#[test]
fn injected_provenance_never_needs_a_git_probe() {
    let calls = Cell::new(0);
    let injected = injected_provenance("0123456789abcdef".to_string(), "true").unwrap();
    assert_eq!(
        injected,
        BuildProvenance::from_injected("0123456789abcdef".to_string(), true).unwrap()
    );
    assert_eq!(calls.get(), 0);
}

#[test]
fn nix_injected_dirty_bit_matches_local_tracked_state_meaning() {
    let clean = injected_provenance("0123456789abcdef".to_string(), "false").unwrap();
    let dirty = injected_provenance("0123456789abcdef".to_string(), "true").unwrap();
    assert!(!clean.dirty);
    assert!(dirty.dirty);
}

#[test]
fn injected_commit_accepts_only_raw_hex_or_unknown() {
    for valid in ["0123456", "0123456789abcdef", "ABCDEF0", "unknown"] {
        assert!(
            BuildProvenance::from_injected(valid.to_string(), false).is_ok(),
            "{valid}"
        );
    }
    for invalid in [
        "",
        "abc123",
        "nix-commit",
        "deadbeef-dirty",
        "deadbeef\n// injected",
        "unknown\n# injected",
    ] {
        assert!(
            BuildProvenance::from_injected(invalid.to_string(), false).is_err(),
            "{invalid:?}"
        );
    }
}

#[test]
fn local_git_watch_paths_cover_head_index_and_resolved_ref() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    run_git(root, &["init"]);
    fs::write(root.join("tracked.txt"), "initial\n").unwrap();
    run_git(root, &["add", "."]);
    commit(root, "initial");
    let paths = git_watch_paths(root).unwrap();
    assert!(paths.iter().any(|path| path.ends_with("HEAD")));
    assert!(paths.iter().any(|path| path.ends_with("index")));
    assert!(
        paths
            .iter()
            .any(|path| path.to_string_lossy().contains("/refs/"))
    );
}

#[test]
fn repository_tracked_dirty_scope_ignores_untracked_and_tracks_all_tracked_changes() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    run_git(root, &["init"]);
    fs::create_dir_all(root.join("crates/symbol/generation")).unwrap();
    fs::write(root.join("API.md"), "initial\n").unwrap();
    fs::write(root.join("unrelated-tracked.txt"), "initial\n").unwrap();
    fs::write(
        root.join("crates/symbol/generation/runtime.rs"),
        "pub const VALUE: u8 = 1;\n",
    )
    .unwrap();
    run_git(root, &["add", "."]);
    commit(root, "initial");

    let initial = discover_git(root).unwrap();
    assert!(!initial.dirty);

    fs::write(root.join("untracked.txt"), "ignored\n").unwrap();
    assert!(!discover_git(root).unwrap().dirty);

    let untracked = root.join("crates/symbol/generation/untracked.rs");
    fs::write(&untracked, "pub const NEW: u8 = 2;\n").unwrap();
    assert!(!discover_git(root).unwrap().dirty);
    fs::remove_file(untracked).unwrap();
    assert!(!discover_git(root).unwrap().dirty);

    fs::write(root.join("unrelated-tracked.txt"), "changed\n").unwrap();
    assert!(discover_git(root).unwrap().dirty);
    run_git(root, &["add", "unrelated-tracked.txt"]);
    assert!(discover_git(root).unwrap().dirty);
    commit(root, "changed");
    let changed = discover_git(root).unwrap();
    assert!(!changed.dirty);
    assert_ne!(changed.commit, initial.commit);
}

fn run_git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(root: &Path, message: &str) {
    run_git(
        root,
        &[
            "-c",
            "user.name=Phase Six Tests",
            "-c",
            "user.email=phase-six@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
}

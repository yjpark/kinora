//! Architectural invariants enforced as plain tests — the mechanism for
//! structural rules that clippy cannot express. This is a virtual workspace, so
//! the test walks every `crates/*/src` tree from the workspace root and asserts
//! the invariant, failing as a red `cargo test` in the normal TDD loop.
//!
//! implements: jig::rust::prefer-file-modules

use std::path::{Path, PathBuf};

/// Recursively collect every file named `mod.rs` under `dir`.
fn find_mod_rs(dir: &Path, hits: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_mod_rs(&path, hits);
        } else if path.file_name().is_some_and(|name| name == "mod.rs") {
            hits.push(path);
        }
    }
}

/// rule: jig::rust::prefer-file-modules — prefer `foo.rs` over `foo/mod.rs`.
#[test]
fn no_mod_rs_files() {
    // crates/kinora -> crates -> workspace root
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root is two levels above the crate manifest");
    let crates = workspace.join("crates");
    let mut hits = Vec::new();
    find_mod_rs(&crates, &mut hits);
    assert!(
        hits.is_empty(),
        "rule jig::rust::prefer-file-modules: use `foo.rs` beside `foo/`, not \
         `foo/mod.rs`. Offending files: {hits:#?}"
    );
}

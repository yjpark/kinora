//! `stencil scaffold <kinograph>`: generate a fresh source file for an
//! api-kinograph — a `stencil:kinograph` binding header followed by one filled
//! `stencil:slot` per entry, in kinograph order.
//!
//! Scaffold is a *first-placement* helper: it produces the initial skeleton an
//! agent then edits freely (moving slots, adding bodies). Subsequent refreshes
//! are `stencil sync`'s job. The generated source is written to stdout so the
//! caller chooses the destination (`stencil scaffold user-api > src/user.rs`);
//! this keeps scaffold from guessing filenames or clobbering existing files.
//!
//! Implementation reuses the engine end to end: build a skeleton (header +
//! empty slots), then run it through [`sync_file`] so the read-only blocks fill
//! by the exact same path `sync` uses. The output therefore re-syncs clean.

use std::fmt::Write as _;
use std::path::Path;

use kinora::paths::kinora_root;
use kinora::resolve::Resolver;
use stencil::engine::{kinograph_slot_names, sync_file};
use stencil::region::StencilFile;
use stencil::target::LanguageTarget;
use stencil::StencilError;

use crate::common::CliError;

/// Generate the scaffold source for the api-kinograph named by `reference`
/// (name or id), rendered with `target`. Returns the source to print.
pub fn run_scaffold(
    repo_root: &Path,
    reference: &str,
    target: &dyn LanguageTarget,
) -> Result<String, CliError> {
    let resolver = Resolver::load(kinora_root(repo_root))?;
    let names = kinograph_slot_names(reference, &resolver)?;

    let skeleton = build_skeleton(reference, &names, target);
    let file = StencilFile::parse(&skeleton, target).map_err(StencilError::from)?;
    let outcome = sync_file(&file, &resolver, target)?;
    Ok(outcome.file.to_source(target))
}

/// Build the skeleton source: a binding header, then one empty `stencil:slot`
/// per entry name, blank-line separated for readability. The engine fills the
/// read-only blocks; the blank `Text` lines survive untouched.
fn build_skeleton(reference: &str, names: &[String], target: &dyn LanguageTarget) -> String {
    let leader = target.comment_leader();
    let mut out = String::new();
    let _ = writeln!(out, "{leader} stencil:kinograph {reference}");
    for name in names {
        let _ = writeln!(out);
        let _ = writeln!(out, "{leader} stencil:slot {name}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use stencil::engine::sync_file as engine_sync;
    use stencil::kinds;
    use stencil::target::RustTarget;
    use tempfile::TempDir;

    const SPEC_MD: &str =
        "Creates a user. Errors if the name is empty.\n\n```rust\npub fn new(name: &str) -> Result<User, UserError>;\n```\n";
    const FIND_MD: &str =
        "Finds a user by id.\n\n```rust\npub fn find(id: u64) -> Option<User>;\n```\n";

    fn params(kind: &str, content: &[u8], name: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: kind.into(),
            content: content.to_vec(),
            author: "t".into(),
            provenance: "t".into(),
            ts: "2026-06-10T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), name.into())]),
            id: None,
            parents: vec![],
        }
    }

    fn store_spec(repo_root: &Path, name: &str, md: &str) -> kinora::event::Event {
        store_kino(&kinora_root(repo_root), params(kinds::API_SPEC, md.as_bytes(), name))
            .unwrap()
            .event
    }

    fn store_kinograph(
        repo_root: &Path,
        name: &str,
        entries: Vec<kinora::kinograph::Entry>,
    ) -> kinora::event::Event {
        let kg = kinora::kinograph::Kinograph { entries };
        let content = kg.to_styxl().unwrap();
        store_kino(&kinora_root(repo_root), params(kinds::API_KINOGRAPH, content.as_bytes(), name))
            .unwrap()
            .event
    }

    #[test]
    fn scaffolds_header_and_one_filled_slot_per_entry_in_order() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        let new = store_spec(root, "user-new", SPEC_MD);
        let find = store_spec(root, "user-find", FIND_MD);
        store_kinograph(
            root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(new.id),
                kinora::kinograph::Entry::with_id(find.id),
            ],
        );

        let src = run_scaffold(root, "user-api", &RustTarget).unwrap();

        // Binding header once, a slot + filled read-only block per entry.
        assert!(src.contains("// stencil:kinograph user-api"));
        assert!(src.contains("// stencil:slot user-new"));
        assert!(src.contains("// stencil:slot user-find"));
        assert!(src.contains("pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(src.contains("pub fn find(id: u64) -> Option<User>;"));
        assert!(src.contains("/// Creates a user. Errors if the name is empty."));

        // Kinograph order is preserved: user-new precedes user-find.
        let new_pos = src.find("stencil:slot user-new").unwrap();
        let find_pos = src.find("stencil:slot user-find").unwrap();
        assert!(new_pos < find_pos, "scaffold must follow kinograph order:\n{src}");
    }

    #[test]
    fn scaffold_output_re_syncs_clean() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        let new = store_spec(root, "user-new", SPEC_MD);
        store_kinograph(root, "user-api", vec![kinora::kinograph::Entry::with_id(new.id)]);

        let src = run_scaffold(root, "user-api", &RustTarget).unwrap();

        // Re-parsing and syncing the scaffold output is a no-op — proof the
        // generated markers are well-formed and idempotent.
        let parsed = StencilFile::parse(&src, &RustTarget).unwrap();
        let resolver = Resolver::load(kinora_root(root)).unwrap();
        let out = engine_sync(&parsed, &resolver, &RustTarget).unwrap();
        assert!(!out.report.changed(), "scaffold output should re-sync clean:\n{src}");
    }

    #[test]
    fn empty_kinograph_yields_header_only() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        store_kinograph(root, "empty-api", vec![]);

        let src = run_scaffold(root, "empty-api", &RustTarget).unwrap();
        assert!(src.contains("// stencil:kinograph empty-api"));
        assert!(!src.contains("stencil:slot"));
    }

    #[test]
    fn unknown_kinograph_errors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();

        let err = run_scaffold(root, "does-not-exist", &RustTarget).unwrap_err();
        assert!(matches!(err, CliError::Stencil(_)), "got: {err:?}");
    }

    #[test]
    fn non_kinograph_kind_errors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        store_kino(&kinora_root(root), params("markdown", b"just docs", "user-api")).unwrap();

        let err = run_scaffold(root, "user-api", &RustTarget).unwrap_err();
        assert!(
            matches!(err, CliError::Stencil(stencil::StencilError::NotApiKinograph { .. })),
            "got: {err:?}"
        );
    }

    #[test]
    fn scaffold_by_id_works() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        let new = store_spec(root, "user-new", SPEC_MD);
        let kg = store_kinograph(root, "user-api", vec![kinora::kinograph::Entry::with_id(new.id)]);

        // Reference the kinograph by id rather than name.
        let src = run_scaffold(root, &kg.id, &RustTarget).unwrap();
        assert!(src.contains("// stencil:slot user-new"));
        assert!(src.contains("pub fn new(name: &str)"));

        // The by-id header (`stencil:kinograph <hash>`) must also re-sync clean.
        let parsed = StencilFile::parse(&src, &RustTarget).unwrap();
        let resolver = Resolver::load(kinora_root(root)).unwrap();
        let out = engine_sync(&parsed, &resolver, &RustTarget).unwrap();
        assert!(!out.report.changed(), "by-id scaffold should re-sync clean:\n{src}");
    }

    #[test]
    fn unslottable_entry_name_errors() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        // A spec whose name has whitespace can't become a single-token slot.
        let spec = store_spec(root, "user new", SPEC_MD);
        store_kinograph(root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        let err = run_scaffold(root, "user-api", &RustTarget).unwrap_err();
        assert!(
            matches!(err, CliError::Stencil(stencil::StencilError::UnslottableEntryName { .. })),
            "got: {err:?}"
        );
    }
}

//! Render kinos and kinographs into an in-memory mdbook-shaped `Book`.
//!
//! The library layer has no knowledge of disk layout, branches, or git. It
//! takes a loaded `Resolver` plus a branch label, and returns a deterministic
//! list of rendered pages. The CLI layer pairs this with disk writes.
//!
//! Kind dispatch (MVP):
//! - `markdown` — content is passed through verbatim
//! - `kinograph` — composed via `Kinograph::render`
//! - `text` — wrapped in a fenced `text` code block
//! - `binary` — replaced with a placeholder note
//! - other kinds — placeholder note naming the kind
//!
//! `kino://<64hex-id>[/]` occurrences in the body are rewritten to relative
//! links to the target page. Unknown ids are left unchanged.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;

use crate::hash::{Hash, SHORTHASH_LEN};
use crate::kinograph::{Kinograph, KinographError};
use crate::resolve::{ResolveError, Resolver};

/// One rendered page in the book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    pub id: String,
    pub slug: String,
    pub branch: String,
    pub title: String,
    pub kind: String,
    pub body: String,
}

/// Identity we chose not to render and why — surfaced so the CLI can tell
/// the user when a fork (or similar) caused silent drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedIdentity {
    pub id: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    MultipleHeads,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::MultipleHeads => {
                write!(f, "multiple heads (fork unresolved by branch tiebreaker)")
            }
        }
    }
}

/// Ordered collection of rendered pages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Book {
    pub pages: Vec<RenderedPage>,
    pub skipped: Vec<SkippedIdentity>,
}

#[derive(Debug)]
pub enum RenderError {
    Io(io::Error),
    Resolve(ResolveError),
    Kinograph(KinographError),
    Utf8(std::string::FromUtf8Error),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Io(e) => write!(f, "render io error: {e}"),
            RenderError::Resolve(e) => write!(f, "{e}"),
            RenderError::Kinograph(e) => write!(f, "{e}"),
            RenderError::Utf8(e) => write!(f, "kino content is not valid UTF-8: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<io::Error> for RenderError {
    fn from(e: io::Error) -> Self {
        RenderError::Io(e)
    }
}

impl From<ResolveError> for RenderError {
    fn from(e: ResolveError) -> Self {
        RenderError::Resolve(e)
    }
}

impl From<KinographError> for RenderError {
    fn from(e: KinographError) -> Self {
        RenderError::Kinograph(e)
    }
}

impl From<std::string::FromUtf8Error> for RenderError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        RenderError::Utf8(e)
    }
}

/// Render every identity's current head into a `Book` labelled by `branch`.
///
/// Pages are sorted by `(name-or-empty, id)` so the output is stable across
/// runs. Identities with no head are skipped silently (shouldn't happen once
/// the ledger is well-formed, but it keeps the renderer robust).
pub fn render_for_branch(
    resolver: &Resolver,
    branch: impl Into<String>,
) -> Result<Book, RenderError> {
    let branch = branch.into();
    let mut entries: Vec<(String, String, String, String)> = Vec::new(); // (name, id, kind, body)
    let mut skipped: Vec<SkippedIdentity> = Vec::new();

    let mut ids: Vec<&String> = resolver.identities().keys().collect();
    ids.sort();

    for id in ids {
        let resolved = match resolver.resolve_by_id(id) {
            Ok(r) => r,
            Err(ResolveError::MultipleHeads { .. }) => {
                skipped.push(SkippedIdentity {
                    id: id.clone(),
                    reason: SkipReason::MultipleHeads,
                });
                continue;
            }
            Err(e) => return Err(RenderError::Resolve(e)),
        };
        let kind = resolved.head.kind.clone();
        let name = resolved
            .head
            .metadata
            .get("name")
            .cloned()
            .unwrap_or_default();
        let body = render_body(&kind, &resolved.content, resolver)?;
        entries.push((name, resolved.id.clone(), kind, body));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let slug_by_id = build_slug_map(&entries);

    let pages = entries
        .into_iter()
        .map(|(name, id, kind, body)| {
            let slug = slug_by_id[&id].clone();
            let body = rewrite_kino_urls(&body, &slug_by_id);
            let title = if name.is_empty() {
                short_id(&id).to_owned()
            } else {
                name.clone()
            };
            RenderedPage {
                id,
                slug,
                branch: branch.clone(),
                title,
                kind,
                body,
            }
        })
        .collect();

    Ok(Book { pages, skipped })
}

fn render_body(
    kind: &str,
    content: &[u8],
    resolver: &Resolver,
) -> Result<String, RenderError> {
    match kind {
        "markdown" => Ok(String::from_utf8(content.to_vec())?),
        "kinograph" => {
            let kinograph = Kinograph::parse(content)?;
            Ok(kinograph.render(resolver)?)
        }
        "text" => {
            let body = String::from_utf8(content.to_vec())?;
            Ok(format!("```text\n{body}\n```\n"))
        }
        "binary" => Ok("> (opaque binary — see source store for bytes)\n".to_owned()),
        other => Ok(format!(
            "> (unrenderable kind `{other}` — no renderer registered)\n"
        )),
    }
}

fn build_slug_map(entries: &[(String, String, String, String)]) -> HashMap<String, String> {
    let mut slugs: HashMap<String, String> = HashMap::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (name, id, _, _) in entries {
        let base = slug_for(name, id);
        let mut candidate = base.clone();
        let mut n = 2;
        while used.contains(&candidate) {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        used.insert(candidate.clone());
        slugs.insert(id.clone(), candidate);
    }
    slugs
}

fn slug_for(name: &str, id: &str) -> String {
    let shorthash = short_id(id);
    if name.is_empty() {
        shorthash.to_owned()
    } else {
        format!("{}-{}", sanitize_slug(name), shorthash)
    }
}

fn short_id(id: &str) -> &str {
    if id.len() >= SHORTHASH_LEN {
        &id[..SHORTHASH_LEN]
    } else {
        id
    }
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        let mapped = match ch {
            'A'..='Z' => Some(ch.to_ascii_lowercase()),
            'a'..='z' | '0'..='9' | '_' | '-' => Some(ch),
            _ => None,
        };
        match mapped {
            Some(c) => {
                out.push(c);
                prev_dash = c == '-';
            }
            None => {
                if !prev_dash && !out.is_empty() {
                    out.push('-');
                    prev_dash = true;
                }
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "kino".to_owned()
    } else {
        out
    }
}

/// Walk `body` and rewrite `kino://<64hex>[/]` occurrences to relative
/// markdown links. Unknown ids are left unchanged so the book still renders.
fn rewrite_kino_urls(body: &str, slug_by_id: &HashMap<String, String>) -> String {
    const PREFIX: &str = "kino://";
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(idx) = rest.find(PREFIX) {
        out.push_str(&rest[..idx]);
        let after_prefix = &rest[idx + PREFIX.len()..];
        if after_prefix.len() >= 64 && after_prefix.is_char_boundary(64) {
            let id_slice = &after_prefix[..64];
            if Hash::from_str(id_slice).is_ok()
                && let Some(slug) = slug_by_id.get(id_slice)
            {
                out.push_str(slug);
                out.push_str(".md");
                let after_id = &after_prefix[64..];
                let skip_slash = if after_id.starts_with('/') { 1 } else { 0 };
                rest = &after_id[skip_slash..];
                continue;
            }
        }
        out.push_str(PREFIX);
        rest = after_prefix;
    }
    out.push_str(rest);
    out
}

/// Write a `Book` to disk in mdbook's expected shape.
///
/// Layout:
/// ```text
/// <cache_root>/
///   book.toml
///   src/
///     SUMMARY.md               # grouped by branch
///     <branch>/
///       index.md               # per-branch landing page
///       <slug>.md              # one per rendered page
/// ```
///
/// `src/` is deleted and rebuilt from scratch on every call so stale pages
/// don't linger. `book.toml` is always rewritten — mdbook's build output
/// (`book/`) is untouched.
pub fn write_book(cache_root: &Path, title: &str, book: &Book) -> Result<(), RenderError> {
    fs::create_dir_all(cache_root)?;

    let src_dir = cache_root.join("src");
    if src_dir.exists() {
        fs::remove_dir_all(&src_dir)?;
    }
    fs::create_dir_all(&src_dir)?;

    fs::write(cache_root.join("book.toml"), book_toml(title))?;

    let by_branch = group_by_branch(book);
    for (branch, pages) in &by_branch {
        let branch_dir = src_dir.join(branch);
        fs::create_dir_all(&branch_dir)?;
        fs::write(branch_dir.join("index.md"), branch_index_md(branch, pages))?;
        for page in pages {
            let body = page_with_source_marker(page);
            fs::write(branch_dir.join(format!("{}.md", page.slug)), body)?;
        }
    }

    fs::write(src_dir.join("SUMMARY.md"), summary_md(&by_branch))?;
    Ok(())
}

fn book_toml(title: &str) -> String {
    let mut escaped = String::with_capacity(title.len());
    for ch in title.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            c if c.is_control() => escaped.push_str(&format!("\\u{:04X}", c as u32)),
            c => escaped.push(c),
        }
    }
    format!(
        "[book]\ntitle = \"{escaped}\"\nsrc = \"src\"\nauthors = [\"kinora\"]\n\n[output.html]\n",
    )
}

fn group_by_branch(book: &Book) -> BTreeMap<String, Vec<&RenderedPage>> {
    let mut map: BTreeMap<String, Vec<&RenderedPage>> = BTreeMap::new();
    for page in &book.pages {
        map.entry(page.branch.clone()).or_default().push(page);
    }
    map
}

fn summary_md(by_branch: &BTreeMap<String, Vec<&RenderedPage>>) -> String {
    let mut out = String::from("# Summary\n\n");
    for (branch, pages) in by_branch {
        out.push_str(&format!("- [{branch}]({branch}/index.md)\n"));
        for page in pages {
            out.push_str(&format!(
                "  - [{}]({}/{}.md)\n",
                page.title, branch, page.slug,
            ));
        }
    }
    out
}

fn branch_index_md(branch: &str, pages: &[&RenderedPage]) -> String {
    let mut out = format!("# {branch}\n\n");
    if pages.is_empty() {
        out.push_str("_(no pages)_\n");
        return out;
    }
    out.push_str("Pages:\n\n");
    for page in pages {
        out.push_str(&format!("- [{}]({}.md)\n", page.title, page.slug));
    }
    out
}

fn page_with_source_marker(page: &RenderedPage) -> String {
    let mut out = page.body.clone();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "\n---\n\n*Rendered from branch `{}` (id `{}`)*\n",
        page.branch,
        short_id(&page.id),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init;
    use crate::kino::{store_kino, StoreKinoParams};
    use crate::paths::kinora_root;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn setup() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn params(kind: &str, content: &[u8], name: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: kind.into(),
            content: content.to_vec(),
            author: "yj".into(),
            provenance: "test".into(),
            ts: "2026-04-18T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), name.into())]),
            id: None,
            parents: vec![],
        }
    }

    #[test]
    fn renders_single_markdown_kino() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"# Hello\n\nBody text.", "greet")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        assert_eq!(book.pages.len(), 1);
        let page = &book.pages[0];
        assert_eq!(page.branch, "main");
        assert_eq!(page.kind, "markdown");
        assert_eq!(page.title, "greet");
        assert!(page.slug.starts_with("greet-"));
        assert!(page.body.contains("# Hello"));
    }

    #[test]
    fn pages_sorted_by_name_then_id_for_stability() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"b", "beta")).unwrap();
        store_kino(&root, params("markdown", b"a", "alpha")).unwrap();
        store_kino(&root, params("markdown", b"c", "charlie")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        let titles: Vec<_> = book.pages.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["alpha", "beta", "charlie"]);
    }

    #[test]
    fn text_kind_wraps_in_fenced_code_block() {
        let (_t, root) = setup();
        store_kino(&root, params("text", b"plain body", "note")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        assert!(book.pages[0].body.starts_with("```text\n"));
        assert!(book.pages[0].body.contains("plain body"));
        assert!(book.pages[0].body.trim_end().ends_with("```"));
    }

    #[test]
    fn binary_kind_emits_placeholder() {
        let (_t, root) = setup();
        store_kino(&root, params("binary", b"\x00\x01\x02", "blob")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        assert!(
            book.pages[0].body.contains("opaque binary"),
            "got body: {}",
            book.pages[0].body
        );
    }

    #[test]
    fn unknown_kind_emits_warning_placeholder() {
        let (_t, root) = setup();
        store_kino(&root, params("mystery::format", b"x", "m")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        assert!(
            book.pages[0].body.contains("unrenderable kind"),
            "got body: {}",
            book.pages[0].body
        );
        assert!(book.pages[0].body.contains("mystery::format"));
    }

    #[test]
    fn kinograph_kind_renders_composed_content() {
        let (_t, root) = setup();
        let a = store_kino(&root, params("markdown", b"alpha", "a")).unwrap();
        let b = store_kino(&root, params("markdown", b"bravo", "b")).unwrap();

        let kg_content = format!("entries ({{id {}}} {{id {}}})", a.event.id, b.event.id);
        store_kino(
            &root,
            params("kinograph", kg_content.as_bytes(), "composed"),
        )
        .unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        let kg_page = book.pages.iter().find(|p| p.kind == "kinograph").unwrap();
        assert!(kg_page.body.contains("alpha"));
        assert!(kg_page.body.contains("bravo"));
    }

    #[test]
    fn kino_url_rewritten_to_relative_md_link() {
        let (_t, root) = setup();
        let target = store_kino(&root, params("markdown", b"target body", "target")).unwrap();
        let referrer_body = format!(
            "See also: [target](kino://{}/) for details.\n",
            target.event.id
        );
        store_kino(
            &root,
            params("markdown", referrer_body.as_bytes(), "referrer"),
        )
        .unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        let referrer_page = book.pages.iter().find(|p| p.title == "referrer").unwrap();
        let target_slug = &book.pages.iter().find(|p| p.title == "target").unwrap().slug;
        assert!(
            referrer_page.body.contains(&format!("{target_slug}.md")),
            "expected body to contain `{target_slug}.md`; got: {}",
            referrer_page.body
        );
        assert!(
            !referrer_page.body.contains("kino://"),
            "kino:// URL should have been rewritten: {}",
            referrer_page.body
        );
    }

    #[test]
    fn kino_url_with_unknown_id_passes_through_untouched() {
        let (_t, root) = setup();
        let bogus = "0".repeat(64);
        let body = format!("broken: [x](kino://{bogus}/)\n");
        store_kino(&root, params("markdown", body.as_bytes(), "x")).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        assert!(book.pages[0].body.contains(&format!("kino://{bogus}/")));
    }

    #[test]
    fn kino_url_without_trailing_slash_also_rewritten() {
        let (_t, root) = setup();
        let target = store_kino(&root, params("markdown", b"t", "target")).unwrap();
        let body = format!("link: kino://{}\n", target.event.id);
        store_kino(&root, params("markdown", body.as_bytes(), "ref")).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        let referrer = book.pages.iter().find(|p| p.title == "ref").unwrap();
        assert!(!referrer.body.contains("kino://"));
    }

    #[test]
    fn empty_repo_yields_empty_book() {
        let (_t, root) = setup();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        assert!(book.pages.is_empty());
    }

    #[test]
    fn branch_label_propagates_to_every_page() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"x", "a")).unwrap();
        store_kino(&root, params("markdown", b"y", "b")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "feature/foo").unwrap();
        assert!(book.pages.iter().all(|p| p.branch == "feature/foo"));
    }

    #[test]
    fn slugs_are_unique_when_names_collide() {
        // Two identities with the same metadata.name — shorthash suffix
        // should keep slugs unique.
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"a", "dup")).unwrap();
        store_kino(&root, params("markdown", b"b", "dup")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        let slugs: std::collections::HashSet<_> =
            book.pages.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs.len(), book.pages.len(), "slugs collided: {book:?}");
    }

    #[test]
    fn write_book_materializes_mdbook_layout() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"# Alpha\n", "alpha")).unwrap();
        store_kino(&root, params("markdown", b"# Beta\n", "beta")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();

        let cache = TempDir::new().unwrap();
        write_book(cache.path(), "My Book", &book).unwrap();

        assert!(cache.path().join("book.toml").is_file());
        assert!(cache.path().join("src/SUMMARY.md").is_file());
        assert!(cache.path().join("src/main/index.md").is_file());

        let summary =
            std::fs::read_to_string(cache.path().join("src/SUMMARY.md")).unwrap();
        assert!(summary.starts_with("# Summary"));
        assert!(summary.contains("- [main](main/index.md)"));
        assert!(summary.contains("main/"));
        for page in &book.pages {
            let path = cache.path().join("src/main").join(format!("{}.md", page.slug));
            assert!(path.is_file(), "missing page: {path:?}");
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(
                contents.contains("Rendered from branch `main`"),
                "expected source marker; got: {contents}"
            );
        }
    }

    #[test]
    fn write_book_rebuilds_src_from_scratch() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"v1", "doc")).unwrap();
        let cache = TempDir::new().unwrap();

        // First render.
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        write_book(cache.path(), "t", &book).unwrap();
        let stale_path = cache.path().join("src/main/stale-page.md");
        std::fs::write(&stale_path, "stale").unwrap();
        assert!(stale_path.exists());

        // Second render — should wipe stale file.
        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();
        write_book(cache.path(), "t", &book).unwrap();
        assert!(!stale_path.exists(), "stale file survived rebuild");
    }

    #[test]
    fn book_toml_escapes_control_chars_in_title() {
        let cache = TempDir::new().unwrap();
        let book = Book::default();
        write_book(cache.path(), "a\nb\"c\\d", &book).unwrap();
        let toml = std::fs::read_to_string(cache.path().join("book.toml")).unwrap();
        assert!(toml.contains(r#"title = "a\nb\"c\\d""#), "got: {toml}");
    }

    #[test]
    fn book_toml_includes_title_and_src_dir() {
        let cache = TempDir::new().unwrap();
        let book = Book::default();
        write_book(cache.path(), "Kinora Cache", &book).unwrap();
        let toml = std::fs::read_to_string(cache.path().join("book.toml")).unwrap();
        assert!(toml.contains("title = \"Kinora Cache\""), "got: {toml}");
        assert!(toml.contains("src = \"src\""), "got: {toml}");
    }

    #[test]
    fn summary_groups_by_branch_in_sorted_order() {
        let book = Book {
            pages: vec![
                RenderedPage {
                    id: "a".repeat(64),
                    slug: "alpha-aaaaaaaa".into(),
                    branch: "zeta".into(),
                    title: "Alpha".into(),
                    kind: "markdown".into(),
                    body: "x".into(),
                },
                RenderedPage {
                    id: "b".repeat(64),
                    slug: "beta-bbbbbbbb".into(),
                    branch: "main".into(),
                    title: "Beta".into(),
                    kind: "markdown".into(),
                    body: "y".into(),
                },
            ],
            skipped: vec![],
        };
        let cache = TempDir::new().unwrap();
        write_book(cache.path(), "t", &book).unwrap();
        let summary =
            std::fs::read_to_string(cache.path().join("src/SUMMARY.md")).unwrap();
        let main_idx = summary.find("[main]").unwrap();
        let zeta_idx = summary.find("[zeta]").unwrap();
        assert!(main_idx < zeta_idx, "branches not sorted: {summary}");
    }

    #[test]
    fn forked_identity_is_surfaced_as_skipped_with_reason() {
        // Under the hot-ledger layout every event has its own file, so a
        // fork is simply two sibling versions off the same parent — no HEAD
        // manipulation needed.
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "forked")).unwrap();

        let mut a = params("markdown", b"left", "forked");
        a.id = Some(birth.event.id.clone());
        a.parents = vec![birth.event.hash.clone()];
        a.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, a).unwrap();

        let mut b = params("markdown", b"right", "forked");
        b.id = Some(birth.event.id.clone());
        b.parents = vec![birth.event.hash.clone()];
        b.ts = "2026-04-18T10:00:02Z".into();
        store_kino(&root, b).unwrap();

        // Independent "clean" identity still renders.
        store_kino(&root, params("markdown", b"ok", "clean")).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let book = render_for_branch(&resolver, "main").unwrap();

        let titles: Vec<_> = book.pages.iter().map(|p| p.title.as_str()).collect();
        assert!(!titles.contains(&"forked"), "forked should be skipped: {titles:?}");
        assert!(titles.contains(&"clean"));

        assert_eq!(book.skipped.len(), 1, "skipped: {:?}", book.skipped);
        assert_eq!(book.skipped[0].id, birth.event.id);
        assert_eq!(book.skipped[0].reason, SkipReason::MultipleHeads);
    }
}

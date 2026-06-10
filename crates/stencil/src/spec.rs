//! The content model of a `kudo::api-spec` kino.
//!
//! A spec kino is a markdown blob: a prose behavioral contract interleaved with
//! one or more fenced ```rust blocks carrying the signatures. [`SpecItem`]
//! splits the two:
//!
//! - `doc_prose` — the contract markdown with the rust code blocks excised
//!   (everything else preserved verbatim, including non-rust fenced examples).
//!   The engine renders this into `///` doc-comments via a [`LanguageTarget`].
//! - `code_fragments` — the inner text of each ```rust block, in document
//!   order. These are the signatures stencil renders into the read-only block.
//!
//! The split is offset-based: pulldown-cmark identifies the byte ranges of the
//! rust code blocks, their inner text becomes the fragments, and the prose is
//! the original markdown minus those ranges. Markdown parsing is total, so
//! [`SpecItem::parse`] is infallible; only the bytes→UTF-8 step
//! ([`SpecItem::from_bytes`]) can fail.
//!
//! [`LanguageTarget`]: crate::target::LanguageTarget

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

/// A parsed `kudo::api-spec` kino: prose contract + signature code fragments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpecItem {
    /// The behavioral contract: markdown with the rust code blocks removed,
    /// normalized (runs of blank lines collapsed, ends trimmed).
    pub doc_prose: String,
    /// The inner text of each ```rust block, in document order. Each fragment
    /// is trimmed of surrounding blank lines.
    pub code_fragments: Vec<String>,
}

impl SpecItem {
    /// Parse a spec kino's markdown content. Infallible — any text is valid
    /// markdown.
    pub fn parse(markdown: &str) -> Self {
        let mut code_fragments: Vec<String> = Vec::new();
        let mut removed: Vec<Range<usize>> = Vec::new();

        // State for the code block currently being walked.
        let mut block_start = 0usize;
        let mut block_is_rust = false;
        let mut block_text = String::new();
        let mut in_code = false;

        for (event, range) in Parser::new(markdown).into_offset_iter() {
            match event {
                Event::Start(Tag::CodeBlock(kind)) => {
                    in_code = true;
                    block_start = range.start;
                    block_text.clear();
                    block_is_rust = match kind {
                        CodeBlockKind::Fenced(info) => info_is_rust(&info),
                        CodeBlockKind::Indented => false,
                    };
                }
                Event::Text(text) if in_code => block_text.push_str(&text),
                Event::End(TagEnd::CodeBlock) => {
                    if block_is_rust {
                        code_fragments.push(block_text.trim_matches('\n').to_owned());
                        // Excise the whole fenced block (open fence → close
                        // fence) from the prose. Start event spans the block;
                        // union with the End range defensively.
                        removed.push(block_start..range.end.max(block_start));
                    }
                    in_code = false;
                }
                _ => {}
            }
        }

        SpecItem {
            doc_prose: stitch_prose(markdown, &removed),
            code_fragments,
        }
    }

    /// Parse from raw kino bytes, decoding UTF-8 first.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SpecError> {
        let text = std::str::from_utf8(bytes).map_err(|_| SpecError::NotUtf8)?;
        Ok(Self::parse(text))
    }

    /// The signature code: all fragments joined by a blank line. Empty when the
    /// spec carries no rust block.
    pub fn code(&self) -> String {
        self.code_fragments.join("\n\n")
    }

    /// Whether the spec carries any signature code.
    pub fn has_code(&self) -> bool {
        !self.code_fragments.is_empty()
    }
}

/// Error from decoding a spec kino's bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("api-spec kino content is not valid UTF-8")]
    NotUtf8,
}

/// A fenced block's info string designates rust iff its first token (split on
/// `,` or whitespace) is exactly `rust` — so `rust`, `rust,ignore`, and
/// `rust,no_run` all count; ` `, `text`, `bash` do not.
fn info_is_rust(info: &str) -> bool {
    info.split([',', ' ', '\t']).next().map(str::trim) == Some("rust")
}

/// Stitch the prose: `markdown` with the rust-block byte `ranges` removed, then
/// trimmed. Ranges come from pulldown-cmark event offsets, so they fall on char
/// boundaries and don't overlap; sort defensively.
///
/// Normalization is *seam-local*: at each join where a block was excised, the
/// surrounding blank lines are collapsed to a single paragraph break. Content
/// inside surviving spans (e.g. a non-rust ```text block with internal blank
/// lines) is preserved byte-for-byte — only the seams are touched.
fn stitch_prose(markdown: &str, ranges: &[Range<usize>]) -> String {
    let mut sorted: Vec<Range<usize>> = ranges.to_vec();
    sorted.sort_by_key(|r| r.start);

    let mut out = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    for r in sorted {
        if r.start > cursor {
            push_segment(&mut out, &markdown[cursor..r.start]);
        }
        cursor = cursor.max(r.end);
    }
    if cursor < markdown.len() {
        push_segment(&mut out, &markdown[cursor..]);
    }
    out.trim().to_owned()
}

/// Append a surviving prose `segment`. The first segment is taken verbatim;
/// every later segment crosses an excision seam, so its blank-line boundary
/// with the prior prose is collapsed to exactly one paragraph break — without
/// disturbing the segment's interior.
fn push_segment(out: &mut String, segment: &str) {
    if out.is_empty() {
        out.push_str(segment);
        return;
    }
    let seg = segment.trim_start_matches('\n');
    if seg.is_empty() {
        return;
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out.push_str("\n\n");
    out.push_str(seg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_then_rust_block_splits_cleanly() {
        let md = "Creates a user. Errors if the name is empty.\n\n```rust\npub fn new(name: &str) -> Result<User, UserError>;\n```\n";
        let item = SpecItem::parse(md);
        assert_eq!(item.doc_prose, "Creates a user. Errors if the name is empty.");
        assert_eq!(
            item.code_fragments,
            vec!["pub fn new(name: &str) -> Result<User, UserError>;"]
        );
    }

    #[test]
    fn rust_only_has_empty_prose() {
        let md = "```rust\npub fn f();\n```\n";
        let item = SpecItem::parse(md);
        assert_eq!(item.doc_prose, "");
        assert_eq!(item.code_fragments, vec!["pub fn f();"]);
    }

    #[test]
    fn prose_only_has_no_code() {
        let md = "Just a contract, no signature yet.\n";
        let item = SpecItem::parse(md);
        assert_eq!(item.doc_prose, "Just a contract, no signature yet.");
        assert!(item.code_fragments.is_empty());
        assert!(!item.has_code());
        assert_eq!(item.code(), "");
    }

    #[test]
    fn empty_input_yields_empty_item() {
        let item = SpecItem::parse("");
        assert_eq!(item, SpecItem::default());
    }

    #[test]
    fn multiple_rust_blocks_kept_in_order() {
        let md = "Doc one.\n\n```rust\npub struct User;\n```\n\nDoc two.\n\n```rust\nimpl User { pub fn new() -> Self; }\n```\n";
        let item = SpecItem::parse(md);
        assert_eq!(
            item.code_fragments,
            vec!["pub struct User;", "impl User { pub fn new() -> Self; }"]
        );
        assert_eq!(item.code(), "pub struct User;\n\nimpl User { pub fn new() -> Self; }");
        // Both prose paragraphs survive, separated by a single blank line.
        assert_eq!(item.doc_prose, "Doc one.\n\nDoc two.");
    }

    #[test]
    fn non_rust_fence_stays_in_prose() {
        let md = "Example output:\n\n```text\nhello\n```\n\n```rust\npub fn hi();\n```\n";
        let item = SpecItem::parse(md);
        assert_eq!(item.code_fragments, vec!["pub fn hi();"]);
        // The ```text block is preserved verbatim in the prose.
        assert!(item.doc_prose.contains("```text"));
        assert!(item.doc_prose.contains("hello"));
    }

    #[test]
    fn non_rust_fence_interior_blank_lines_preserved_verbatim() {
        // Seam-local normalization must not touch content inside a surviving
        // non-rust block, even with 2+ consecutive internal blank lines.
        let md = "```text\nline a\n\n\nline b\n```\n\n```rust\npub fn f();\n```\n";
        let item = SpecItem::parse(md);
        assert_eq!(item.code_fragments, vec!["pub fn f();"]);
        assert!(
            item.doc_prose.contains("line a\n\n\nline b"),
            "interior blank lines altered: {:?}",
            item.doc_prose
        );
    }

    #[test]
    fn rust_with_attributes_is_treated_as_rust() {
        for info in ["rust,ignore", "rust,no_run", "rust, should_panic"] {
            let md = format!("Doc.\n\n```{info}\npub fn f();\n```\n");
            let item = SpecItem::parse(&md);
            assert_eq!(item.code_fragments, vec!["pub fn f();"], "info={info}");
            assert_eq!(item.doc_prose, "Doc.", "info={info}");
        }
    }

    #[test]
    fn plain_unlabeled_fence_is_not_rust() {
        let md = "```\nnot rust\n```\n";
        let item = SpecItem::parse(md);
        assert!(item.code_fragments.is_empty());
        assert!(item.doc_prose.contains("not rust"));
    }

    #[test]
    fn indented_code_block_is_not_treated_as_rust() {
        // A 4-space indented block is an indented code block, not a fenced rust
        // block — it stays in the prose.
        let md = "Doc.\n\n    let x = 1;\n";
        let item = SpecItem::parse(md);
        assert!(item.code_fragments.is_empty());
        assert!(item.doc_prose.contains("let x = 1;"));
    }

    #[test]
    fn prose_before_and_after_code_is_stitched() {
        let md = "Before.\n\n```rust\npub fn f();\n```\n\nAfter.\n";
        let item = SpecItem::parse(md);
        assert_eq!(item.doc_prose, "Before.\n\nAfter.");
        assert_eq!(item.code_fragments, vec!["pub fn f();"]);
    }

    #[test]
    fn multiline_signature_preserved_exactly() {
        let md = "Doc.\n\n```rust\npub fn new(\n    name: &str,\n    age: u8,\n) -> Result<User, UserError>;\n```\n";
        let item = SpecItem::parse(md);
        assert_eq!(
            item.code_fragments,
            vec!["pub fn new(\n    name: &str,\n    age: u8,\n) -> Result<User, UserError>;"]
        );
    }

    #[test]
    fn from_bytes_decodes_utf8() {
        let item = SpecItem::from_bytes(b"Doc.\n\n```rust\npub fn f();\n```\n").unwrap();
        assert_eq!(item.doc_prose, "Doc.");
        assert_eq!(item.code_fragments, vec!["pub fn f();"]);
    }

    #[test]
    fn from_bytes_rejects_invalid_utf8() {
        let err = SpecItem::from_bytes(&[0xff, 0xfe]).unwrap_err();
        assert_eq!(err, SpecError::NotUtf8);
    }

    #[test]
    fn doc_prose_preserves_markdown_formatting() {
        let md = "# Heading\n\nA paragraph with **bold** and a list:\n\n- one\n- two\n\n```rust\npub fn f();\n```\n";
        let item = SpecItem::parse(md);
        assert!(item.doc_prose.contains("# Heading"));
        assert!(item.doc_prose.contains("**bold**"));
        assert!(item.doc_prose.contains("- one"));
    }
}

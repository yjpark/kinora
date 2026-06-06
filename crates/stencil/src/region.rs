//! The on-disk structure of a stencil-managed source file, and the line-based
//! parser/serializer that round-trips it.
//!
//! A target file is an ordered sequence of [`Block`]s:
//!
//! - [`Block::Text`] — verbatim editable content stencil never touches.
//! - [`Block::Binding`] — the `stencil:kinograph <ref>` line binding the file
//!   to an api-kinograph.
//! - [`Block::Slot`] — an agent-placed `stencil:slot <name>` anchor.
//! - [`Block::ReadOnly`] — a `stencil:ro <name> <hash>` … `stencil:end` block;
//!   its inner content is stencil-authored.
//!
//! Markers are line-based (silp ethos — stencil never parses the host
//! language); the only language-specific input is the comment leader, supplied
//! by a [`LanguageTarget`]. Marker lines are normalized to a canonical spelling
//! on serialize; editable [`Block::Text`] content is preserved byte-for-byte.
//! A file stencil itself wrote round-trips `parse → to_source → parse`
//! byte-stable.

use crate::target::LanguageTarget;

/// Marker directive keywords (the token after `stencil:`).
const DIR_KINOGRAPH: &str = "kinograph";
const DIR_SLOT: &str = "slot";
const DIR_RO: &str = "ro";
const DIR_END: &str = "end";

/// One structural block of a stencil-managed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Verbatim editable lines (no trailing line terminators stored). Preserved
    /// byte-for-byte across a round-trip.
    Text { lines: Vec<String> },
    /// `stencil:kinograph <reference>` — binds the file to an api-kinograph by
    /// name or id.
    Binding { indent: String, reference: String },
    /// `stencil:slot <name>` — an agent-placed anchor naming the api-kinograph
    /// entry whose read-only block belongs here.
    Slot { indent: String, name: String },
    /// `stencil:ro <name> <hash>` … `stencil:end`. `content` is the inner,
    /// stencil-authored lines (excludes the two marker lines). `hash` is the
    /// resolved spec-kino content hash (empty string if absent).
    ReadOnly {
        indent: String,
        name: String,
        hash: String,
        content: Vec<String>,
    },
}

impl Block {
    /// Build a read-only block from rendered `content` lines.
    pub fn read_only(
        name: impl Into<String>,
        hash: impl Into<String>,
        content: Vec<String>,
        indent: impl Into<String>,
    ) -> Self {
        Block::ReadOnly {
            indent: indent.into(),
            name: name.into(),
            hash: hash.into(),
            content,
        }
    }

    /// Render this block back to its source lines (no terminators), using
    /// `leader` for marker lines.
    pub fn to_lines(&self, leader: &str) -> Vec<String> {
        match self {
            Block::Text { lines } => lines.clone(),
            Block::Binding { indent, reference } => {
                vec![marker(indent, leader, DIR_KINOGRAPH, &[reference])]
            }
            Block::Slot { indent, name } => {
                vec![marker(indent, leader, DIR_SLOT, &[name])]
            }
            Block::ReadOnly { indent, name, hash, content } => {
                let open = if hash.is_empty() {
                    marker(indent, leader, DIR_RO, &[name])
                } else {
                    marker(indent, leader, DIR_RO, &[name, hash])
                };
                let mut out = Vec::with_capacity(content.len() + 2);
                out.push(open);
                out.extend(content.iter().cloned());
                out.push(marker(indent, leader, DIR_END, &[]));
                out
            }
        }
    }
}

/// A parsed stencil-managed source file: an ordered list of [`Block`]s.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StencilFile {
    pub blocks: Vec<Block>,
}

/// Errors from [`StencilFile::parse`]. Line numbers are 1-based.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("line {line}: `stencil:ro` block opened here was never closed with `stencil:end`")]
    UnterminatedReadOnly { line: usize },
    #[error("line {line}: `stencil:end` with no open `stencil:ro` block")]
    UnexpectedEnd { line: usize },
    #[error("line {line}: `stencil:ro` nested inside an open read-only block")]
    NestedReadOnly { line: usize },
    #[error("line {line}: malformed `stencil:{directive}` marker: {reason}")]
    Malformed {
        line: usize,
        directive: String,
        reason: String,
    },
}

/// A recognized marker line, split into directive + whitespace-delimited args.
struct Marker<'a> {
    indent: &'a str,
    directive: &'a str,
    args: Vec<&'a str>,
}

/// Recognize `<indent><leader> stencil:<directive> <args…>`. Returns `None` for
/// any line that is not a stencil marker (including ordinary comments and
/// doc-comments — `///foo` does not start with the bare leader followed by
/// `stencil:`).
fn parse_marker<'a>(line: &'a str, leader: &str) -> Option<Marker<'a>> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let rest = trimmed.strip_prefix(leader)?;
    // Require whitespace (or end) between the leader and `stencil:` so `///x`
    // (leader `//`) isn't mistaken for a marker: after stripping `//` we get
    // `/x`, which does not trim to a `stencil:` prefix.
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("stencil:")?;
    let mut it = rest.split_whitespace();
    let directive = it.next()?;
    let args: Vec<&str> = it.collect();
    Some(Marker { indent, directive, args })
}

fn marker(indent: &str, leader: &str, directive: &str, args: &[&str]) -> String {
    let mut s = format!("{indent}{leader} stencil:{directive}");
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

struct OpenRo {
    indent: String,
    name: String,
    hash: String,
    content: Vec<String>,
    start_line: usize,
}

impl StencilFile {
    /// Parse `input` into structural blocks using `target`'s comment leader.
    pub fn parse(input: &str, target: &dyn LanguageTarget) -> Result<Self, ParseError> {
        let leader = target.comment_leader();
        let mut blocks: Vec<Block> = Vec::new();
        let mut text: Vec<String> = Vec::new();
        let mut open: Option<OpenRo> = None;

        let flush_text = |text: &mut Vec<String>, blocks: &mut Vec<Block>| {
            if !text.is_empty() {
                blocks.push(Block::Text { lines: std::mem::take(text) });
            }
        };

        for (idx, line) in input.split('\n').enumerate() {
            let lineno = idx + 1;
            let m = parse_marker(line, leader);

            if open.is_some() {
                match &m {
                    Some(mk) if mk.directive == DIR_END => {
                        if !mk.args.is_empty() {
                            return Err(ParseError::Malformed {
                                line: lineno,
                                directive: DIR_END.to_owned(),
                                reason: "`stencil:end` takes no arguments".to_owned(),
                            });
                        }
                        let open_ro = open.take().unwrap();
                        blocks.push(Block::ReadOnly {
                            indent: open_ro.indent,
                            name: open_ro.name,
                            hash: open_ro.hash,
                            content: open_ro.content,
                        });
                    }
                    Some(mk) if mk.directive == DIR_RO => {
                        return Err(ParseError::NestedReadOnly { line: lineno });
                    }
                    // Any other line (including stray non-`ro` markers) is
                    // stencil-owned read-only content.
                    _ => open.as_mut().unwrap().content.push(line.to_owned()),
                }
                continue;
            }

            let Some(mk) = m else {
                text.push(line.to_owned());
                continue;
            };

            match mk.directive {
                DIR_KINOGRAPH => {
                    let reference = exactly_one(&mk, lineno, "<reference>")?;
                    flush_text(&mut text, &mut blocks);
                    blocks.push(Block::Binding {
                        indent: mk.indent.to_owned(),
                        reference,
                    });
                }
                DIR_SLOT => {
                    let name = exactly_one(&mk, lineno, "<name>")?;
                    flush_text(&mut text, &mut blocks);
                    blocks.push(Block::Slot {
                        indent: mk.indent.to_owned(),
                        name,
                    });
                }
                DIR_RO => {
                    if mk.args.is_empty() || mk.args.len() > 2 {
                        return Err(ParseError::Malformed {
                            line: lineno,
                            directive: DIR_RO.to_owned(),
                            reason: "expected `stencil:ro <name> [hash]`".to_owned(),
                        });
                    }
                    flush_text(&mut text, &mut blocks);
                    open = Some(OpenRo {
                        indent: mk.indent.to_owned(),
                        name: mk.args[0].to_owned(),
                        hash: mk.args.get(1).copied().unwrap_or("").to_owned(),
                        content: Vec::new(),
                        start_line: lineno,
                    });
                }
                DIR_END => {
                    return Err(ParseError::UnexpectedEnd { line: lineno });
                }
                other => {
                    return Err(ParseError::Malformed {
                        line: lineno,
                        directive: other.to_owned(),
                        reason: "unknown stencil directive".to_owned(),
                    });
                }
            }
        }

        if let Some(open_ro) = open {
            return Err(ParseError::UnterminatedReadOnly { line: open_ro.start_line });
        }
        flush_text(&mut text, &mut blocks);
        Ok(Self { blocks })
    }

    /// Render the file back to source text using `target`'s comment leader.
    /// Inverse of [`parse`](Self::parse) for canonical input.
    pub fn to_source(&self, target: &dyn LanguageTarget) -> String {
        let leader = target.comment_leader();
        let mut lines: Vec<String> = Vec::new();
        for block in &self.blocks {
            lines.extend(block.to_lines(leader));
        }
        lines.join("\n")
    }

    /// The api-kinograph reference this file is bound to, if it declares one
    /// (the first `stencil:kinograph` binding).
    pub fn binding(&self) -> Option<&str> {
        self.blocks.iter().find_map(|b| match b {
            Block::Binding { reference, .. } => Some(reference.as_str()),
            _ => None,
        })
    }

    /// The slot names declared in this file, in document order.
    pub fn slot_names(&self) -> Vec<&str> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Slot { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }
}

fn exactly_one(mk: &Marker, line: usize, placeholder: &str) -> Result<String, ParseError> {
    match mk.args.as_slice() {
        [one] => Ok((*one).to_owned()),
        _ => Err(ParseError::Malformed {
            line,
            directive: mk.directive.to_owned(),
            reason: format!("expected exactly one argument {placeholder}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::RustTarget;

    fn parse(input: &str) -> Result<StencilFile, ParseError> {
        StencilFile::parse(input, &RustTarget)
    }

    /// Assert `parse → to_source` reproduces the input byte-for-byte.
    fn assert_roundtrip(input: &str) {
        let f = parse(input).expect("parses");
        assert_eq!(f.to_source(&RustTarget), input, "round-trip changed bytes");
    }

    #[test]
    fn empty_input_roundtrips() {
        assert_roundtrip("");
    }

    #[test]
    fn plain_text_with_no_markers_roundtrips() {
        assert_roundtrip("fn main() {\n    println!(\"hi\");\n}\n");
    }

    #[test]
    fn blank_lines_and_trailing_newline_preserved() {
        assert_roundtrip("a\n\n\nb\n");
        assert_roundtrip("a\n\n\nb");
    }

    #[test]
    fn binding_is_parsed_and_roundtrips() {
        let input = "// stencil:kinograph my-crate-api\n";
        let f = parse(input).unwrap();
        assert_eq!(f.binding(), Some("my-crate-api"));
        assert_roundtrip(input);
    }

    #[test]
    fn slot_is_parsed_and_roundtrips() {
        let input = "// stencil:slot user-new\n";
        let f = parse(input).unwrap();
        assert_eq!(f.slot_names(), vec!["user-new"]);
        assert_roundtrip(input);
    }

    #[test]
    fn read_only_block_is_parsed_and_roundtrips() {
        let input =
            "// stencil:ro user-new abc123\n/// Creates a user.\npub fn new() -> User;\n// stencil:end\n";
        let f = parse(input).unwrap();
        match &f.blocks[0] {
            Block::ReadOnly { name, hash, content, indent } => {
                assert_eq!(name, "user-new");
                assert_eq!(hash, "abc123");
                assert_eq!(indent, "");
                assert_eq!(content, &["/// Creates a user.", "pub fn new() -> User;"]);
            }
            other => panic!("expected ReadOnly, got {other:?}"),
        }
        assert_roundtrip(input);
    }

    #[test]
    fn read_only_without_hash_parses_and_roundtrips() {
        let input = "// stencil:ro user-new\npub fn new();\n// stencil:end\n";
        let f = parse(input).unwrap();
        match &f.blocks[0] {
            Block::ReadOnly { hash, .. } => assert_eq!(hash, ""),
            other => panic!("expected ReadOnly, got {other:?}"),
        }
        assert_roundtrip(input);
    }

    #[test]
    fn empty_read_only_block_roundtrips() {
        let input = "// stencil:ro x abc\n// stencil:end\n";
        let f = parse(input).unwrap();
        match &f.blocks[0] {
            Block::ReadOnly { content, .. } => assert!(content.is_empty()),
            other => panic!("expected ReadOnly, got {other:?}"),
        }
        assert_roundtrip(input);
    }

    #[test]
    fn indented_markers_preserve_indentation_and_roundtrip() {
        let input = concat!(
            "mod user {\n",
            "    // stencil:slot user-new\n",
            "    // stencil:ro user-new abc\n",
            "    pub fn new() -> User;\n",
            "    // stencil:end\n",
            "}\n",
        );
        let f = parse(input).unwrap();
        // Indent captured on both slot and ro.
        let indents: Vec<&str> = f
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Slot { indent, .. } => Some(indent.as_str()),
                Block::ReadOnly { indent, .. } => Some(indent.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(indents, vec!["    ", "    "]);
        assert_roundtrip(input);
    }

    #[test]
    fn full_mixed_file_roundtrips_byte_for_byte() {
        let input = concat!(
            "// stencil:kinograph user-api\n",
            "use crate::error::UserError;\n",
            "\n",
            "// stencil:slot user-new\n",
            "// stencil:ro user-new deadbeef\n",
            "/// Creates a user. Errors if the name is empty.\n",
            "pub fn new(name: &str) -> Result<User, UserError>;\n",
            "// stencil:end\n",
            "{\n",
            "    // editable body the agent writes\n",
            "    todo!()\n",
            "}\n",
        );
        assert_roundtrip(input);
    }

    #[test]
    fn ordinary_comment_is_text_not_a_marker() {
        let input = "// just a comment\n// stencil:slot s\n";
        let f = parse(input).unwrap();
        assert!(matches!(f.blocks[0], Block::Text { .. }));
        assert!(matches!(f.blocks[1], Block::Slot { .. }));
        assert_roundtrip(input);
    }

    #[test]
    fn doc_comment_line_is_text_not_a_marker() {
        // `///` starts with the `//` leader, but `/ stencil:` != `stencil:`.
        let input = "/// stencil:slot looks-like-marker\n";
        let f = parse(input).unwrap();
        assert_eq!(f.blocks.len(), 1);
        assert!(matches!(f.blocks[0], Block::Text { .. }));
        assert_roundtrip(input);
    }

    #[test]
    fn multiple_slots_listed_in_order() {
        let input = "// stencil:slot a\n// stencil:slot b\n// stencil:slot c\n";
        let f = parse(input).unwrap();
        assert_eq!(f.slot_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn unterminated_read_only_errors_with_start_line() {
        let input = "x\n// stencil:ro user-new abc\npub fn new();\n";
        let err = parse(input).unwrap_err();
        assert_eq!(err, ParseError::UnterminatedReadOnly { line: 2 });
    }

    #[test]
    fn end_without_open_block_errors() {
        let input = "x\n// stencil:end\n";
        let err = parse(input).unwrap_err();
        assert_eq!(err, ParseError::UnexpectedEnd { line: 2 });
    }

    #[test]
    fn nested_read_only_errors() {
        let input = "// stencil:ro a x\n// stencil:ro b y\n// stencil:end\n// stencil:end\n";
        let err = parse(input).unwrap_err();
        assert_eq!(err, ParseError::NestedReadOnly { line: 2 });
    }

    #[test]
    fn slot_without_name_errors() {
        let input = "// stencil:slot\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "slot"));
    }

    #[test]
    fn slot_with_extra_args_errors() {
        let input = "// stencil:slot a b\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "slot"));
    }

    #[test]
    fn kinograph_without_reference_errors() {
        let input = "// stencil:kinograph\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "kinograph"));
    }

    #[test]
    fn ro_without_name_errors() {
        let input = "// stencil:ro\n// stencil:end\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "ro"));
    }

    #[test]
    fn ro_with_too_many_args_errors() {
        let input = "// stencil:ro a b c\n// stencil:end\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "ro"));
    }

    #[test]
    fn unknown_directive_errors() {
        let input = "// stencil:frobnicate x\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "frobnicate"));
    }

    #[test]
    fn end_with_args_errors() {
        let input = "// stencil:ro a x\n// stencil:end extra\n";
        let err = parse(input).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { directive, .. } if directive == "end"));
    }

    #[test]
    fn block_read_only_constructor_emits_expected_lines() {
        let block = Block::read_only(
            "user-new",
            "abc",
            vec!["/// doc".to_owned(), "pub fn new();".to_owned()],
            "",
        );
        assert_eq!(
            block.to_lines("//"),
            vec![
                "// stencil:ro user-new abc",
                "/// doc",
                "pub fn new();",
                "// stencil:end",
            ]
        );
    }

    // Markers are stencil-owned: non-canonical hand-edited marker spelling is
    // deliberately normalized on serialize (only editable Text is byte-stable).
    // These two tests pin that intent.

    #[test]
    fn non_canonical_marker_whitespace_is_normalized() {
        let f = parse("//   stencil:slot s\n").unwrap();
        assert_eq!(f.to_source(&RustTarget), "// stencil:slot s\n");
    }

    #[test]
    fn mismatched_end_indent_is_normalized_to_open_indent() {
        // `end` at column 0, `ro` indented → `end` is re-indented to match.
        let input = "    // stencil:ro a h\n    code;\n// stencil:end\n";
        let f = parse(input).unwrap();
        assert_eq!(
            f.to_source(&RustTarget),
            "    // stencil:ro a h\n    code;\n    // stencil:end\n"
        );
    }

    #[test]
    fn reparse_of_serialized_is_structurally_identical() {
        let input = concat!(
            "// stencil:kinograph api\n",
            "// stencil:slot a\n",
            "// stencil:ro a h\n",
            "code;\n",
            "// stencil:end\n",
            "editable\n",
        );
        let f1 = parse(input).unwrap();
        let f2 = parse(&f1.to_source(&RustTarget)).unwrap();
        assert_eq!(f1, f2);
    }
}

//! Language targets: the per-language knowledge stencil needs to render
//! read-only sections. The engine and the region (de)serializer are
//! language-agnostic — everything language-specific lives behind
//! [`LanguageTarget`]. Bootstrap ships [`RustTarget`]; other languages are
//! additive (RFC-0004 principle 3) with no engine redesign.

/// What stencil needs to know about a target language to render markers and
/// doc contracts into its source.
pub trait LanguageTarget {
    /// The line-comment leader that prefixes every stencil marker, e.g. `//`
    /// for Rust. Markers are spelled `<leader> stencil:<directive> …`.
    fn comment_leader(&self) -> &str;

    /// Render a prose behavioral contract as a doc-comment block for this
    /// language. Each input line maps to one output line; the result has no
    /// trailing newline (callers compose it with the signature code).
    fn doc_comment(&self, prose: &str) -> String;
}

/// The Rust language target: `//` markers, `///` doc-comments.
#[derive(Debug, Default, Clone, Copy)]
pub struct RustTarget;

impl LanguageTarget for RustTarget {
    fn comment_leader(&self) -> &str {
        "//"
    }

    fn doc_comment(&self, prose: &str) -> String {
        prose
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    "///".to_owned()
                } else {
                    format!("/// {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_comment_leader_is_double_slash() {
        assert_eq!(RustTarget.comment_leader(), "//");
    }

    #[test]
    fn doc_comment_prefixes_single_line() {
        assert_eq!(RustTarget.doc_comment("Creates a user."), "/// Creates a user.");
    }

    #[test]
    fn doc_comment_prefixes_each_line() {
        let prose = "Creates a user.\nErrors if the name is empty.";
        assert_eq!(
            RustTarget.doc_comment(prose),
            "/// Creates a user.\n/// Errors if the name is empty."
        );
    }

    #[test]
    fn doc_comment_renders_blank_line_without_trailing_space() {
        let prose = "first\n\nsecond";
        assert_eq!(RustTarget.doc_comment(prose), "/// first\n///\n/// second");
    }

    #[test]
    fn doc_comment_empty_prose_is_a_single_bare_doc_marker() {
        assert_eq!(RustTarget.doc_comment(""), "///");
    }
}

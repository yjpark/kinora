use facet::Facet;
use figue::{self as args, FigueBuiltins};

#[derive(Facet, Debug)]
pub struct Cli {
    /// Operate on the kinora repo rooted at this path instead of walking up
    /// from the current directory. `.kinora/` must exist directly under it.
    #[facet(args::named, args::short = 'C', default)]
    pub repo_root: Option<String>,

    #[facet(args::subcommand)]
    pub command: Command,

    #[facet(flatten)]
    pub builtins: FigueBuiltins,
}

#[derive(Facet, Debug)]
#[repr(u8)]
#[allow(dead_code)]
pub enum Command {
    /// Render API spec into the read-only sections of source files. Scans the
    /// given paths (files or directories) for stencil markers, resolves each
    /// slot against its file's bound api-kinograph, and rewrites the read-only
    /// blocks in place — editable regions are preserved.
    ///
    /// Engine + behavior land in kinora-hgpl / kinora-exay.
    Sync {
        /// Files or directories to scan. Defaults to the current directory.
        #[facet(args::positional, default)]
        paths: Vec<String>,
    },

    /// Generate a new source file from an api-kinograph: a `stencil:kinograph`
    /// header plus one filled `stencil:slot` per entry, in kinograph order.
    ///
    /// Behavior lands in kinora-guv8.
    Scaffold {
        /// The api-kinograph to scaffold from, by name or id.
        #[facet(args::positional)]
        kinograph: String,
    },
}

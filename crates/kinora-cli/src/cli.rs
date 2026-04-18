use facet::Facet;
use figue::{self as args, FigueBuiltins};

#[derive(Facet, Debug)]
pub struct Cli {
    #[facet(args::subcommand)]
    pub command: Command,

    #[facet(flatten)]
    pub builtins: FigueBuiltins,
}

#[derive(Facet, Debug)]
#[repr(u8)]
#[allow(dead_code)]
pub enum Command {
    /// Store content as a kino: writes the blob to the content store
    /// (deduped by hash) and appends a ledger event.
    Store {
        /// Kind of kino (e.g. `markdown`, `text`, `binary`, `kinograph`,
        /// or a `prefix::extension` namespaced kind).
        #[facet(args::positional)]
        kind: String,

        /// Path to a file to read content from; reads stdin if omitted.
        #[facet(args::positional, default)]
        path: Option<String>,

        /// Provenance: where does this content come from?
        #[facet(args::named)]
        provenance: String,

        /// Human-readable name, stored in metadata.
        #[facet(args::named, default)]
        name: Option<String>,

        /// Kino identity hash. Omit for a birth event; pass for a version
        /// that links to an existing identity.
        #[facet(args::named, default)]
        id: Option<String>,

        /// Comma-separated list of parent content hashes for version
        /// events.
        #[facet(args::named, default)]
        parents: Option<String>,

        /// Mark this version as a draft (sets `draft=true` in metadata).
        #[facet(args::named, default)]
        draft: bool,

        /// Override author (defaults to `user.name` from git config).
        #[facet(args::named, default)]
        author: Option<String>,

        /// Additional metadata `KEY=VALUE`; repeatable.
        #[facet(args::named, args::short = 'm', default)]
        metadata: Vec<String>,
    },
}

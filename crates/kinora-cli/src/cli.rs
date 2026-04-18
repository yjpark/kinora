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

    /// Render the repo's kinos and kinographs into an mdbook project under
    /// `~/.cache/kinora/<shorthash>-<name>/` (or `$XDG_CACHE_HOME` if set).
    Render {
        /// Override the cache root. Defaults to
        /// `$XDG_CACHE_HOME/kinora/<shorthash>-<name>/` (falling back to
        /// `$HOME/.cache/kinora/<shorthash>-<name>/`).
        #[facet(args::named, default)]
        cache_dir: Option<String>,
    },

    /// Resolve a kino by name or id and print its current content to
    /// stdout. Refuses forks unless `--version HASH` or `--all-heads` is
    /// passed.
    Resolve {
        /// Either a 64-hex identity hash or a metadata `name`.
        #[facet(args::positional)]
        name_or_id: String,

        /// Return the content at a specific version hash instead of the
        /// current head.
        #[facet(args::named, default)]
        version: Option<String>,

        /// On a fork, list all heads instead of erroring.
        #[facet(args::named, default)]
        all_heads: bool,
    },
}

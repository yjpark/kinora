use facet::Facet;
use figue::{self as args, FigueBuiltins};

#[derive(Facet, Debug)]
pub struct Cli {
    /// Operate on the kinora repo rooted at this path instead of walking
    /// up from the current directory. `.kinora/` must exist directly
    /// under it; no walk-up is performed.
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

        /// Root to assign this kino to, as an atomic store+assign pair.
        /// Omit to write only the store event — commit will route the
        /// kino to the `inbox` root implicitly (phase 3.5).
        #[facet(args::named, default)]
        root: Option<String>,
    },

    /// Assign a kino to a named root. Writes a standalone `assign` event
    /// to the staged ledger; commit (phase 3.5) consumes it to decide which
    /// kinos render into which root.
    Assign {
        /// Either a 64-hex identity hash or a metadata `name`.
        #[facet(args::positional)]
        kino: String,

        /// Target root name.
        #[facet(args::positional)]
        root: String,

        /// Comma-separated list of prior assign-event hashes this one
        /// supersedes. Optional.
        #[facet(args::named, default)]
        resolves: Option<String>,

        /// Override author (defaults to `user.name` from git config).
        #[facet(args::named, default)]
        author: Option<String>,

        /// Provenance of this assign. Defaults to `assign`.
        #[facet(args::named, default)]
        provenance: Option<String>,
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

    /// Commit every root declared in `.kinora/config.styx` into a new
    /// `kind: root` kinograph version. Reads every event under
    /// `.kinora/staged/`, picks the head per identity per root, and stores
    /// a canonical root blob per root, updating the pointer at
    /// `.kinora/roots/<name>`. Per-root errors don't short-circuit
    /// sibling roots — clean roots still advance. Exit is non-zero iff
    /// any root errored.
    Commit {
        /// Override author (defaults to `user.name` from git config).
        #[facet(args::named, default)]
        author: Option<String>,

        /// Provenance of this commit run. Defaults to `commit`.
        #[facet(args::named, default)]
        provenance: Option<String>,
    },

    /// Migrate legacy `.styx`-wrapped kinograph blobs to the styxl
    /// one-entry-per-line format. Walks reachable kinograph kinos from each
    /// root's current head, stages new-version events for regular
    /// kinographs, and rewrites root blobs + pointers in place. Idempotent
    /// on repos whose blobs are already styxl. Run `kinora commit`
    /// afterwards to promote the staged versions to heads.
    Reformat {
        /// Override author (defaults to `user.name` from git config).
        #[facet(args::named, default)]
        author: Option<String>,

        /// Provenance of this reformat run. Defaults to `reformat`.
        #[facet(args::named, default)]
        provenance: Option<String>,
    },

    /// Commit every declared root, clone the `.kinora/` directory into a
    /// sibling `.kinora.repack-tmp/`, then atomically swap the two dirs
    /// and delete the old copy. Drops unreachable blobs and rewrites
    /// legacy extensionless filenames into the canonical form. Refuses
    /// to run when a prior `.kinora.repack-tmp` or `.kinora.repack-old`
    /// directory lingers — that state means a previous repack crashed
    /// and needs manual attention.
    Repack {
        /// Override author (defaults to `user.name` from git config).
        #[facet(args::named, default)]
        author: Option<String>,

        /// Provenance of this repack run. Defaults to `repack`.
        #[facet(args::named, default)]
        provenance: Option<String>,
    },

    /// Rebuild a `.kinora/` directory into a fresh target. Copies only
    /// reachable blobs from `<src>` into `<dst>` through the current store
    /// API, rewriting legacy extensionless filenames into the canonical
    /// `<hash>.<ext>` form and dropping unreachable blobs. Hash-preserving:
    /// content bytes are never rewritten (use `kinora reformat` for that).
    /// Both paths are taken verbatim as `.kinora/` directories — no
    /// walk-up.
    Clone {
        /// Source `.kinora/` directory.
        #[facet(args::positional)]
        src: String,

        /// Destination path. Must be empty or non-existent.
        #[facet(args::positional)]
        dst: String,

        /// Override author. Defaults to `clone`.
        #[facet(args::named, default)]
        author: Option<String>,

        /// Provenance of this clone run. Defaults to `clone`.
        #[facet(args::named, default)]
        provenance: Option<String>,
    },
}

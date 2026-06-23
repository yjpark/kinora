//! Stencil — a kinora-backed, silp-inspired crate-API preprocessor.
//!
//! Stencil renders a crate's public API — held in kinora as content-addressed
//! kinos — into read-only sections of source files, leaving the editable
//! regions (function bodies, helpers, tests) untouched. It is the substrate
//! RFC-0004 (kudo) specifies: a structural boundary between API *contract*
//! (kinora-managed, append-only) and *implementation* (in-source, freely
//! edited).
//!
//! This crate is the library; [`stencil-cli`](../stencil_cli/index.html) is the
//! `stencil` binary that drives it. Stencil depends on the `kinora` crate as a
//! library — it reuses kinora's content store, ledger, [`Resolver`] and
//! composition [`Kinograph`] for resolution, and adds a new *renderer* whose
//! target is source files rather than mdbook.
//!
//! # Kind conventions
//!
//! Stencil reserves two namespaced kinora kinds (see [`kinds`]). Both are plain
//! `prefix::name` kinds, already accepted by kinora's namespace validation — no
//! kinora-crate change is required to use them.
//!
//! - [`kinds::API_SPEC`] (`kudo::api-spec`) — a single spec kino: an atomic
//!   element of the public surface (a shared error type, an ID newtype, a
//!   trait) or a logical-unit (a struct with its constructors, a trait with its
//!   contracts). Its content is markdown: a prose behavioral contract followed
//!   by one or more fenced ```rust blocks carrying the signatures.
//! - [`kinds::API_KINOGRAPH`] (`kudo::api-kinograph`) — the per-crate
//!   composition that references the spec kinos making up a crate's API. Its
//!   content is the same `{id, name?, pin?, note?}` styxl entry shape as a
//!   kinora composition [`Kinograph`], so it parses via
//!   `kinora::kinograph::Kinograph::parse`.
//!
//! [`Resolver`]: kinora::resolve::Resolver
//! [`Kinograph`]: kinora::kinograph::Kinograph

pub mod engine;
pub mod kinds;
pub mod region;
pub mod spec;
pub mod target;

// The crate's base error type is stencil-managed (dogfood, kinora-3guj): the
// `StencilError` enum renders into the read-only block below from the
// `stencil-lib-api` api-kinograph. Run `stencil sync` to refresh it.

// stencil:kinograph stencil-lib-api

// stencil:slot stencil-error
// stencil:ro stencil-error 0f708ba665f14f2207397e2c3cb6cbaec25986043a2446d55399e340030b7ad6
/// Base error type for the stencil library. Follows kinora's convention:
/// libraries use `thiserror`, the CLI wraps these in `rootcause` reports.
///
/// Grows as the engine lands (kinora-q28s/thow/hgpl); at scaffolding time it
/// carries the foundational `From` conversions for the kinora errors stencil
/// builds on.
#[derive(Debug, thiserror::Error)]
pub enum StencilError {
    #[error("stencil io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Parse(#[from] region::ParseError),
    #[error(transparent)]
    Spec(#[from] spec::SpecError),
    #[error(transparent)]
    Resolve(#[from] kinora::resolve::ResolveError),
    #[error(transparent)]
    Kinograph(#[from] kinora::kinograph::KinographError),
    #[error("file declares stencil slots but no `stencil:kinograph` binding")]
    NoBinding,
    #[error("binding `{reference}` resolves to kind `{kind}`, not an api-kinograph (`kudo::api-kinograph`)")]
    NotApiKinograph { reference: String, kind: String },
    #[error("api-kinograph entry `{name}` resolves to kind `{kind}`, not an api-spec (`kudo::api-spec`)")]
    NotApiSpec { name: String, kind: String },
    #[error("api-kinograph has two entries named `{name}`; names must be unique to match slots")]
    DuplicateEntryName { name: String },
    #[error("api-kinograph entry name `{name}` cannot be a stencil slot: slot names must be a single token with no whitespace")]
    UnslottableEntryName { name: String },
}
// stencil:end

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

pub mod kinds;
pub mod region;
pub mod spec;
pub mod target;

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
}

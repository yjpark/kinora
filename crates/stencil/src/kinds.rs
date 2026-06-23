//! Reserved kinora kinds for stencil's API-spec workflow.
//!
//! Both are namespaced (`kudo::…`) kinds. Kinora accepts any well-formed
//! `prefix::name` kind, so these need no registration in the kinora crate —
//! they are a *convention* owned here. See the crate-level docs for the content
//! shape each kind carries.

// This module's public API is stencil-managed (dogfood, kinora-3guj): the two
// reserved-kind consts render into the read-only blocks below from the
// `stencil-kinds-api` api-kinograph. Run `stencil sync` to refresh them.

// stencil:kinograph stencil-kinds-api

// stencil:slot kinds-api-spec
// stencil:ro kinds-api-spec 1844db77b976bd8e60c862ccc1411a402196defec2d6f61093584a048a311a91
/// A single spec kino: one element of a crate's public surface (atomic or
/// logical-unit). Content is markdown — prose contract + fenced ```rust
/// signature blocks.
pub const API_SPEC: &str = "kudo::api-spec";
// stencil:end

// stencil:slot kinds-api-kinograph
// stencil:ro kinds-api-kinograph f07ec4c9ff1412eb2eab412824cefc82c0e3a852e2c7989fdc257a1a97cdc990
/// The per-crate API composition: references the spec kinos that make up a
/// crate's API. Content is the kinora composition entry shape (styxl
/// `{id, name?, pin?, note?}` lines), parsed via
/// `kinora::kinograph::Kinograph`.
pub const API_KINOGRAPH: &str = "kudo::api-kinograph";
// stencil:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_kinds_have_expected_spelling() {
        assert_eq!(API_SPEC, "kudo::api-spec");
        assert_eq!(API_KINOGRAPH, "kudo::api-kinograph");
    }

    #[test]
    fn reserved_kinds_pass_kinora_namespace_validation() {
        // The whole point of the namespaced spelling: kinora accepts them as
        // valid kinds with no core change.
        kinora::namespace::validate_kind(API_SPEC).expect("api-spec is a valid kind");
        kinora::namespace::validate_kind(API_KINOGRAPH).expect("api-kinograph is a valid kind");
    }

    #[test]
    fn reserved_kinds_are_namespaced_not_bare() {
        assert!(kinora::namespace::is_namespaced(API_SPEC));
        assert!(kinora::namespace::is_namespaced(API_KINOGRAPH));
    }
}

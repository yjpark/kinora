# Task: Pilot a real published crate end-to-end

**Goal.** Apply the stencil workflow (RFC bootstrap step 4) to one real crate,
beyond the self-dogfood — candidate: the kinora-extracted shared content-store +
ledger crate.

**Realizes:**
- kino://5898fe2453ef8824f71670df106512d00502edbc90ac0fa8705a5d03e6f98c25 — Stencil: a kinora-native, language-agnostic crate-API preprocessor.

**Done when:** a real crate's public API is authored as `kudo::api-spec` kinos +
an api-kinograph, its source is stencil-managed, `stencil sync` re-runs to 0
changed, and the open question of where that crate lives (kinora repo / kudo
`crates/` / moco) is resolved.

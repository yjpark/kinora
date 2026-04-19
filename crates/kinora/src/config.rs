//! Repo-level config for a `.kinora/` tree. On-disk shape is styx; we
//! parse via facet_styx into a private raw struct, then validate/normalize
//! into the public `Config` type.
//!
//! Per-root policies (`Never`, `MaxAge(_)`, `KeepLastN(_)`) are declared
//! in a `roots {}` block. Two roots are auto-provisioned by the library
//! if the user hasn't declared them:
//!
//! - `inbox` — default `MaxAge("30d")` policy (per xi21 §6); nudges triage.
//! - `commits` — default `Never` policy; holds per-commit archive kinos
//!   so the staged ledger can be pruned without losing provenance.
//!
//! User-declared policies for these roots are preserved (never clobbered).
//! Users cannot remove them — the library re-injects on load.

use std::collections::BTreeMap;

use facet::Facet;

/// Retention policy for a single root. Drives commit-time GC in
/// hxmw-6; this bean only lands the declarative primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootPolicy {
    /// Never drop anything from this root.
    Never,
    /// Drop entries older than the named duration. The raw string (e.g.
    /// `"30d"`, `"12h"`) is kept verbatim — actual duration parsing is
    /// deferred to hxmw-6.
    MaxAge(String),
    /// Keep only the N most recent content versions per kino id.
    KeepLastN(usize),
}

/// Aggressive default for `inbox` per xi21 §6 — nudges users to triage.
pub const DEFAULT_INBOX_POLICY: &str = "30d";

impl RootPolicy {
    /// Parse a policy string from config.styx. Grammar:
    ///
    /// - `"never"` → `Never`
    /// - `"keep-last-<N>"` where N is a usize → `KeepLastN(N)`
    /// - `<digits><letters>` (e.g. `"30d"`, `"12h"`) → `MaxAge(raw)`
    /// - anything else → `None`
    pub fn from_policy_str(raw: &str) -> Option<Self> {
        if raw == "never" {
            return Some(RootPolicy::Never);
        }
        if let Some(rest) = raw.strip_prefix("keep-last-") {
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: usize = rest.parse().ok()?;
            return Some(RootPolicy::KeepLastN(n));
        }
        let digit_end = raw.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digit_end == 0 || digit_end == raw.len() {
            return None;
        }
        let tail = &raw.as_bytes()[digit_end..];
        if tail.len() != 1 || !b"smhdwy".contains(&tail[0]) {
            return None;
        }
        Some(RootPolicy::MaxAge(raw.to_owned()))
    }

    /// Inverse of `from_policy_str` — produces the canonical wire form.
    pub fn to_policy_str(&self) -> String {
        match self {
            RootPolicy::Never => "never".to_owned(),
            RootPolicy::MaxAge(s) => s.clone(),
            RootPolicy::KeepLastN(n) => format!("keep-last-{n}"),
        }
    }

    /// For `MaxAge(raw)`, return the duration in seconds. Grammar matches
    /// `from_policy_str`: a non-empty digit prefix followed by one of
    /// `s|m|h|d|w|y`. Calendar-agnostic: `y = 365d`, `w = 7d`, `d = 24h`,
    /// `h = 60m`, `m = 60s`. Returns `None` for `Never` / `KeepLastN`, or
    /// if the raw string is malformed (shouldn't happen — `from_policy_str`
    /// already validated it, but be defensive).
    pub fn max_age_seconds(&self) -> Option<i64> {
        let raw = match self {
            RootPolicy::MaxAge(s) => s,
            _ => return None,
        };
        let digit_end = raw.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digit_end == 0 || digit_end == raw.len() {
            return None;
        }
        let n: i64 = raw[..digit_end].parse().ok()?;
        let unit = raw.as_bytes().get(digit_end).copied()?;
        let factor: i64 = match unit {
            b's' => 1,
            b'm' => 60,
            b'h' => 60 * 60,
            b'd' => 24 * 60 * 60,
            b'w' => 7 * 24 * 60 * 60,
            b'y' => 365 * 24 * 60 * 60,
            _ => return None,
        };
        n.checked_mul(factor)
    }
}

/// Raw on-disk shape of the `roots { <name> { policy "<s>" } ... }` block.
#[derive(Facet, Debug, Clone, PartialEq, Eq)]
struct RawRootBlock {
    policy: String,
}

/// Raw on-disk config. `roots` is `Option<_>` so repos pre-dating the
/// `roots {}` block still parse.
#[derive(Facet, Debug, Clone, PartialEq)]
struct RawConfig {
    #[facet(rename = "repo-url")]
    repo_url: String,
    roots: Option<BTreeMap<String, RawRootBlock>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub repo_url: String,
    pub roots: BTreeMap<String, RootPolicy>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse config.styx: {0}")]
    Parse(String),
    #[error("failed to serialize config: {0}")]
    Serialize(String),
    #[error("invalid policy for root `{root}`: `{raw}` (expected \"never\", \"<N>[smhdwy]\", or \"keep-last-<N>\")")]
    InvalidPolicy { root: String, raw: String },
}

impl Config {
    pub fn from_styx(input: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = facet_styx::from_str(input)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        let mut roots: BTreeMap<String, RootPolicy> = BTreeMap::new();
        if let Some(raw_roots) = raw.roots {
            for (name, block) in raw_roots {
                let policy = RootPolicy::from_policy_str(&block.policy).ok_or_else(|| {
                    ConfigError::InvalidPolicy {
                        root: name.clone(),
                        raw: block.policy.clone(),
                    }
                })?;
                roots.insert(name, policy);
            }
        }
        roots
            .entry("inbox".to_owned())
            .or_insert_with(|| RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()));
        roots
            .entry("commits".to_owned())
            .or_insert(RootPolicy::Never);
        Ok(Self { repo_url: raw.repo_url, roots })
    }

    pub fn to_styx(&self) -> Result<String, ConfigError> {
        let raw_roots: BTreeMap<String, RawRootBlock> = self
            .roots
            .iter()
            .map(|(name, policy)| {
                (
                    name.clone(),
                    RawRootBlock { policy: policy.to_policy_str() },
                )
            })
            .collect();
        let raw = RawConfig {
            repo_url: self.repo_url.clone(),
            roots: Some(raw_roots),
        };
        facet_styx::to_string(&raw).map_err(|e| ConfigError::Serialize(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_auto_provisions() -> Config {
        Config {
            repo_url: "https://github.com/edger-dev/kinora".into(),
            roots: BTreeMap::from([
                (
                    "commits".to_owned(),
                    RootPolicy::Never,
                ),
                (
                    "inbox".to_owned(),
                    RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()),
                ),
            ]),
        }
    }

    // ------------------------------------------------------------------
    // Policy string grammar
    // ------------------------------------------------------------------

    #[test]
    fn policy_parses_never() {
        assert_eq!(RootPolicy::from_policy_str("never"), Some(RootPolicy::Never));
    }

    #[test]
    fn policy_parses_keep_last_n() {
        assert_eq!(
            RootPolicy::from_policy_str("keep-last-10"),
            Some(RootPolicy::KeepLastN(10))
        );
        assert_eq!(
            RootPolicy::from_policy_str("keep-last-1"),
            Some(RootPolicy::KeepLastN(1))
        );
    }

    #[test]
    fn policy_parses_max_age_durations() {
        assert_eq!(
            RootPolicy::from_policy_str("30d"),
            Some(RootPolicy::MaxAge("30d".into()))
        );
        assert_eq!(
            RootPolicy::from_policy_str("12h"),
            Some(RootPolicy::MaxAge("12h".into()))
        );
        assert_eq!(
            RootPolicy::from_policy_str("7d"),
            Some(RootPolicy::MaxAge("7d".into()))
        );
    }

    #[test]
    fn policy_rejects_bogus_input() {
        assert!(RootPolicy::from_policy_str("").is_none());
        assert!(RootPolicy::from_policy_str("forever").is_none());
        assert!(RootPolicy::from_policy_str("30").is_none(), "digits only");
        assert!(RootPolicy::from_policy_str("d").is_none(), "letters only");
        assert!(RootPolicy::from_policy_str("30d5h").is_none(), "compound not yet supported");
        assert!(RootPolicy::from_policy_str("keep-last-").is_none(), "no count");
        assert!(RootPolicy::from_policy_str("keep-last-x").is_none(), "non-numeric count");
        assert!(RootPolicy::from_policy_str("keep-last-+5").is_none(), "signed count");
        assert!(RootPolicy::from_policy_str("Never").is_none(), "keyword case-sensitive");
        assert!(RootPolicy::from_policy_str("30D").is_none(), "unit case-sensitive");
        assert!(RootPolicy::from_policy_str("30qqq").is_none(), "unit not in [smhdwy]");
    }

    #[test]
    fn policy_to_str_roundtrips_each_variant() {
        let cases = [
            RootPolicy::Never,
            RootPolicy::MaxAge("30d".into()),
            RootPolicy::KeepLastN(5),
        ];
        for p in cases {
            let s = p.to_policy_str();
            assert_eq!(RootPolicy::from_policy_str(&s), Some(p.clone()));
        }
    }

    // ------------------------------------------------------------------
    // from_styx / to_styx
    // ------------------------------------------------------------------

    #[test]
    fn roundtrip_minimal_config_injects_auto_provision_defaults() {
        let c = Config {
            repo_url: "https://github.com/edger-dev/kinora".into(),
            roots: BTreeMap::new(),
        };
        let s = c.to_styx().unwrap();
        let parsed = Config::from_styx(&s).unwrap();
        assert_eq!(parsed, cfg_with_auto_provisions());
    }

    #[test]
    fn parses_inline_repo_url_with_no_roots_block_auto_provisions_inbox_and_commits() {
        let c = Config::from_styx("repo-url https://github.com/edger-dev/kinora").unwrap();
        assert_eq!(c.repo_url, "https://github.com/edger-dev/kinora");
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()))
        );
        assert_eq!(c.roots.get("commits"), Some(&RootPolicy::Never));
        assert_eq!(c.roots.len(), 2);
    }

    #[test]
    fn parses_multi_root_config() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  inbox   { policy "30d" }
  rfcs    { policy "never" }
  designs { policy "keep-last-10" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge("30d".into()))
        );
        assert_eq!(c.roots.get("rfcs"), Some(&RootPolicy::Never));
        assert_eq!(
            c.roots.get("designs"),
            Some(&RootPolicy::KeepLastN(10))
        );
    }

    #[test]
    fn auto_provisions_inbox_when_roots_block_present_but_lacks_inbox() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(c.roots.len(), 3);
        assert_eq!(c.roots.get("rfcs"), Some(&RootPolicy::Never));
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()))
        );
        assert_eq!(c.roots.get("commits"), Some(&RootPolicy::Never));
    }

    #[test]
    fn auto_provisions_commits_with_never_policy_when_absent() {
        let c = Config::from_styx("repo-url https://github.com/edger-dev/kinora").unwrap();
        assert_eq!(c.roots.get("commits"), Some(&RootPolicy::Never));
    }

    #[test]
    fn auto_provisions_commits_when_roots_block_present_but_lacks_commits() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(c.roots.get("commits"), Some(&RootPolicy::Never));
    }

    #[test]
    fn explicit_commits_policy_is_preserved_not_overridden() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  commits { policy "keep-last-100" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(
            c.roots.get("commits"),
            Some(&RootPolicy::KeepLastN(100)),
            "user's explicit commits policy must not be clobbered"
        );
    }

    #[test]
    fn explicit_inbox_policy_is_preserved_not_overridden() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  inbox { policy "7d" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge("7d".into())),
            "user's explicit inbox policy must not be clobbered"
        );
    }

    #[test]
    fn invalid_policy_string_reported_with_root_and_raw() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  rfcs { policy "bogus" }
}
"#;
        let err = Config::from_styx(input).unwrap_err();
        match err {
            ConfigError::InvalidPolicy { root, raw } => {
                assert_eq!(root, "rfcs");
                assert_eq!(raw, "bogus");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_multi_root_config() {
        let c = Config {
            repo_url: "https://github.com/edger-dev/kinora".into(),
            roots: BTreeMap::from([
                ("commits".into(), RootPolicy::Never),
                ("inbox".into(), RootPolicy::MaxAge("30d".into())),
                ("rfcs".into(), RootPolicy::Never),
                ("designs".into(), RootPolicy::KeepLastN(10)),
            ]),
        };
        let s = c.to_styx().unwrap();
        let parsed = Config::from_styx(&s).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn serialized_multi_root_config_contains_all_names() {
        let c = Config {
            repo_url: "https://example.com/foo.git".into(),
            roots: BTreeMap::from([
                ("inbox".into(), RootPolicy::MaxAge("30d".into())),
                ("rfcs".into(), RootPolicy::Never),
            ]),
        };
        let s = c.to_styx().unwrap();
        assert!(s.contains("repo-url"), "got: {s}");
        assert!(s.contains("roots"), "got: {s}");
        assert!(s.contains("inbox"), "got: {s}");
        assert!(s.contains("rfcs"), "got: {s}");
        assert!(s.contains("30d"), "got: {s}");
        assert!(s.contains("never"), "got: {s}");
    }
}

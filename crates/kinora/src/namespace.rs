pub const NAMESPACE_SEPARATOR: &str = "::";

/// Bare metadata keys reserved for Kinora core. Any bare key outside this
/// set is rejected on write; extensions must use `prefix::name`.
pub const RESERVED_METADATA_KEYS: &[&str] = &[
    "name",
    "title",
    "description",
    "draft",
    "tags",
    "links",
    "entry_notes",
];

/// Bare values reserved for the ledger event `kind` field. Any bare kind
/// outside this set is rejected on write; extensions must use `prefix::name`.
pub const RESERVED_KINDS: &[&str] = &[
    "markdown",
    "text",
    "binary",
    "kinograph",
];

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NamespaceError {
    UnknownBareKey(String),
    EmptyNamespace,
    EmptyName,
}

impl std::fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NamespaceError::UnknownBareKey(k) => {
                write!(f, "unknown bare key `{k}`; use `prefix::{k}` for extensions")
            }
            NamespaceError::EmptyNamespace => write!(f, "empty namespace prefix before `::`"),
            NamespaceError::EmptyName => write!(f, "empty name after `::`"),
        }
    }
}

impl std::error::Error for NamespaceError {}

pub fn is_namespaced(key: &str) -> bool {
    key.contains(NAMESPACE_SEPARATOR)
}

/// Validate a namespaced key's shape (non-empty prefix and name around `::`).
/// Does not check the prefix against any allowlist — extensions are open.
pub fn validate_namespaced(key: &str) -> Result<(), NamespaceError> {
    if let Some((prefix, name)) = key.split_once(NAMESPACE_SEPARATOR) {
        if prefix.is_empty() {
            Err(NamespaceError::EmptyNamespace)
        } else if name.is_empty() {
            Err(NamespaceError::EmptyName)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

pub fn validate_metadata_key(key: &str) -> Result<(), NamespaceError> {
    if is_namespaced(key) {
        validate_namespaced(key)
    } else if RESERVED_METADATA_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(NamespaceError::UnknownBareKey(key.to_owned()))
    }
}

/// Extension to append when writing a blob of the given `kind`, for UX
/// parity with the underlying content. Extensionless blobs are legacy and
/// still readable via [`crate::paths::find_blob_path`] — the extension is
/// advisory, not a key.
///
/// Returns `None` for kinds that should stay extensionless on disk (`binary`
/// today; bare opaque bytes). Namespaced kinds fall back to `bin` — the
/// store can't know the real extension, and `bin` is a neutral signal that
/// the byte stream shape is up to the owning namespace.
pub fn ext_for_kind(kind: &str) -> Option<&'static str> {
    if is_namespaced(kind) {
        return Some("bin");
    }
    match kind {
        "markdown" => Some("md"),
        "text" => Some("txt"),
        "kinograph" => Some("styx"),
        "binary" => None,
        _ => None,
    }
}

pub fn validate_kind(kind: &str) -> Result<(), NamespaceError> {
    if is_namespaced(kind) {
        validate_namespaced(kind)
    } else if RESERVED_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(NamespaceError::UnknownBareKey(kind.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_reserved_metadata_keys_accepted() {
        for k in RESERVED_METADATA_KEYS {
            assert!(validate_metadata_key(k).is_ok(), "{k}");
        }
    }

    #[test]
    fn unknown_bare_metadata_key_rejected() {
        let err = validate_metadata_key("random").unwrap_err();
        assert_eq!(err, NamespaceError::UnknownBareKey("random".into()));
    }

    #[test]
    fn namespaced_metadata_key_accepted() {
        assert!(validate_metadata_key("kudo::diagram").is_ok());
        assert!(validate_metadata_key("user::sketch").is_ok());
    }

    #[test]
    fn empty_namespace_rejected() {
        assert_eq!(
            validate_metadata_key("::name").unwrap_err(),
            NamespaceError::EmptyNamespace
        );
    }

    #[test]
    fn empty_name_rejected() {
        assert_eq!(
            validate_metadata_key("prefix::").unwrap_err(),
            NamespaceError::EmptyName
        );
    }

    #[test]
    fn reserved_kinds_accepted() {
        for k in RESERVED_KINDS {
            assert!(validate_kind(k).is_ok(), "{k}");
        }
    }

    #[test]
    fn unknown_bare_kind_rejected() {
        assert!(validate_kind("random").is_err());
    }

    #[test]
    fn namespaced_kind_accepted() {
        assert!(validate_kind("kudo::diagram").is_ok());
    }

    #[test]
    fn is_namespaced_detects_separator() {
        assert!(!is_namespaced("name"));
        assert!(is_namespaced("kudo::diagram"));
    }

    #[test]
    fn ext_for_kind_maps_reserved_kinds() {
        assert_eq!(ext_for_kind("markdown"), Some("md"));
        assert_eq!(ext_for_kind("text"), Some("txt"));
        assert_eq!(ext_for_kind("kinograph"), Some("styx"));
        assert_eq!(ext_for_kind("binary"), None);
    }

    #[test]
    fn ext_for_kind_falls_back_to_bin_for_namespaced() {
        assert_eq!(ext_for_kind("team::priority"), Some("bin"));
        assert_eq!(ext_for_kind("kudo::diagram"), Some("bin"));
    }

    #[test]
    fn ext_for_kind_unknown_bare_returns_none() {
        assert_eq!(ext_for_kind("random"), None);
    }
}

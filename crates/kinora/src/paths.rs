use std::path::{Path, PathBuf};

use crate::hash::Hash;

pub const KINORA_DIR: &str = ".kinora";
pub const CONFIG_FILE: &str = "config.styx";
pub const HEAD_FILE: &str = "HEAD";
pub const STORE_DIR: &str = "store";
pub const LEDGER_DIR: &str = "ledger";
pub const LEDGER_EXT: &str = "jsonl";
pub const HOT_DIR: &str = "hot";
pub const HOT_EXT: &str = "jsonl";

pub fn kinora_root(repo_root: &Path) -> PathBuf {
    repo_root.join(KINORA_DIR)
}

pub fn config_path(kinora_root: &Path) -> PathBuf {
    kinora_root.join(CONFIG_FILE)
}

pub fn head_path(kinora_root: &Path) -> PathBuf {
    kinora_root.join(HEAD_FILE)
}

pub fn store_dir(kinora_root: &Path) -> PathBuf {
    kinora_root.join(STORE_DIR)
}

pub fn ledger_dir(kinora_root: &Path) -> PathBuf {
    kinora_root.join(LEDGER_DIR)
}

/// Path for a blob, extensionless. The legacy layout — still used for the
/// `binary` kind and for existing repos written before the extension was
/// introduced. Readers must fall back to [`find_blob_path`] to locate a blob
/// regardless of whether an extension is present.
pub fn store_blob_path(kinora_root: &Path, hash: &Hash) -> PathBuf {
    store_dir(kinora_root).join(hash.shard()).join(hash.as_hex())
}

/// Path for a blob with an optional extension. `ext` is the stem extension
/// (e.g. `md`, `txt`, `styx`) without a leading dot. `None` yields the same
/// extensionless path as [`store_blob_path`].
pub fn store_blob_path_with_ext(
    kinora_root: &Path,
    hash: &Hash,
    ext: Option<&str>,
) -> PathBuf {
    let stem = hash.as_hex();
    let filename = match ext {
        Some(e) => format!("{stem}.{e}"),
        None => stem.to_owned(),
    };
    store_dir(kinora_root).join(hash.shard()).join(filename)
}

/// Find the on-disk path for a blob by hash, regardless of extension.
///
/// Scans the hash's shard directory for any entry whose file stem matches
/// the full 64-hex hash. Returns the first match. Returns `None` if the
/// shard dir is absent or no matching file is present.
///
/// Extensions are advisory — the canonical handle is the hash — so a reader
/// must accept any extension (or none). This is what makes the "first writer
/// wins on extension" dedup semantics viable.
pub fn find_blob_path(kinora_root: &Path, hash: &Hash) -> Option<PathBuf> {
    let shard = store_dir(kinora_root).join(hash.shard());
    let hex = hash.as_hex();
    let entries = std::fs::read_dir(&shard).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        let stem = s.split_once('.').map(|(l, _)| l).unwrap_or(s);
        if stem == hex {
            return Some(shard.join(s));
        }
    }
    None
}

pub fn ledger_file_path(kinora_root: &Path, shorthash: &str) -> PathBuf {
    ledger_dir(kinora_root).join(format!("{shorthash}.{LEDGER_EXT}"))
}

pub fn hot_dir(kinora_root: &Path) -> PathBuf {
    kinora_root.join(HOT_DIR)
}

/// One-file-per-event layout: `.kinora/hot/<ab>/<event-hash>.jsonl`.
/// Sharded by first two hex chars of the event hash (matches the store layout).
pub fn hot_event_path(kinora_root: &Path, event_hash: &Hash) -> PathBuf {
    hot_dir(kinora_root)
        .join(event_hash.shard())
        .join(format!("{}.{HOT_EXT}", event_hash.as_hex()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    #[test]
    fn kinora_root_appends_dotdir() {
        assert_eq!(kinora_root(&root()), PathBuf::from("/repo/.kinora"));
    }

    #[test]
    fn config_at_expected_path() {
        let kin = kinora_root(&root());
        assert_eq!(config_path(&kin), PathBuf::from("/repo/.kinora/config.styx"));
    }

    #[test]
    fn head_at_expected_path() {
        let kin = kinora_root(&root());
        assert_eq!(head_path(&kin), PathBuf::from("/repo/.kinora/HEAD"));
    }

    #[test]
    fn store_and_ledger_dirs() {
        let kin = kinora_root(&root());
        assert_eq!(store_dir(&kin), PathBuf::from("/repo/.kinora/store"));
        assert_eq!(ledger_dir(&kin), PathBuf::from("/repo/.kinora/ledger"));
    }

    #[test]
    fn store_blob_is_sharded() {
        let kin = kinora_root(&root());
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        assert_eq!(
            store_blob_path(&kin, &hash),
            PathBuf::from(
                "/repo/.kinora/store/af/af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            )
        );
    }

    #[test]
    fn ledger_file_uses_shorthash() {
        let kin = kinora_root(&root());
        assert_eq!(
            ledger_file_path(&kin, "af1349b9"),
            PathBuf::from("/repo/.kinora/ledger/af1349b9.jsonl")
        );
    }

    #[test]
    fn hot_dir_is_hot_subdir() {
        let kin = kinora_root(&root());
        assert_eq!(hot_dir(&kin), PathBuf::from("/repo/.kinora/hot"));
    }

    #[test]
    fn store_blob_path_with_ext_appends_dot_ext() {
        let kin = kinora_root(&root());
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        assert_eq!(
            store_blob_path_with_ext(&kin, &hash, Some("md")),
            PathBuf::from(
                "/repo/.kinora/store/af/af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262.md"
            )
        );
    }

    #[test]
    fn store_blob_path_with_ext_none_matches_legacy() {
        let kin = kinora_root(&root());
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        assert_eq!(
            store_blob_path_with_ext(&kin, &hash, None),
            store_blob_path(&kin, &hash)
        );
    }

    #[test]
    fn find_blob_path_returns_none_when_shard_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kin = tmp.path().to_path_buf();
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        assert_eq!(find_blob_path(&kin, &hash), None);
    }

    #[test]
    fn find_blob_path_locates_extensionless_blob() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kin = tmp.path().to_path_buf();
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        let p = store_blob_path(&kin, &hash);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(find_blob_path(&kin, &hash).as_deref(), Some(p.as_path()));
    }

    #[test]
    fn find_blob_path_locates_blob_with_extension() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kin = tmp.path().to_path_buf();
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        let p = store_blob_path_with_ext(&kin, &hash, Some("md"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(find_blob_path(&kin, &hash).as_deref(), Some(p.as_path()));
    }

    #[test]
    fn find_blob_path_ignores_unrelated_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kin = tmp.path().to_path_buf();
        let hash: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        let shard = store_dir(&kin).join(hash.shard());
        std::fs::create_dir_all(&shard).unwrap();
        std::fs::write(shard.join("aabbccddeeff.md"), b"x").unwrap();
        assert_eq!(find_blob_path(&kin, &hash), None);
    }

    #[test]
    fn hot_event_path_shards_by_first_two_hex() {
        let kin = kinora_root(&root());
        let h: Hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
            .parse()
            .unwrap();
        assert_eq!(
            hot_event_path(&kin, &h),
            PathBuf::from(
                "/repo/.kinora/hot/af/af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262.jsonl"
            )
        );
    }
}

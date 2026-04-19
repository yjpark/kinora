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

pub fn store_blob_path(kinora_root: &Path, hash: &Hash) -> PathBuf {
    store_dir(kinora_root).join(hash.shard()).join(hash.as_hex())
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

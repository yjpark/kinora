use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::hash::{Hash, HashParseError};
use crate::namespace::ext_for_kind;
use crate::paths::{find_blob_path, store_blob_path_with_ext, store_dir};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("content store io error: {0}")]
    Io(#[from] io::Error),
    #[error("content hash mismatch at {}: expected {expected}, got {got}", .path.display())]
    HashMismatch { expected: Hash, got: Hash, path: PathBuf },
    #[error("invalid hash in stored path {}: {err}", .path.display())]
    InvalidStoredHash {
        path: PathBuf,
        #[source]
        err: HashParseError,
    },
}

pub struct ContentStore {
    kinora_root: PathBuf,
}

impl ContentStore {
    pub fn new(kinora_root: impl Into<PathBuf>) -> Self {
        Self { kinora_root: kinora_root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.kinora_root
    }

    /// Write `content` to the store, tagging the on-disk filename with the
    /// extension derived from `kind` (e.g. `markdown` → `<hash>.md`). Dedup
    /// semantics are unchanged: the hash of the content is the identity; if
    /// a blob with that hash already exists under any extension, no new file
    /// is written and the existing path stands. Extensions are advisory UX
    /// — the authoritative `kind` lives in the ledger event.
    ///
    /// Tmp path is prefixed (`.tmp-<hash>.<ext>`) so a crashed write never
    /// masquerades as a real blob when [`find_blob_path`] scans the shard
    /// dir — the stem-is-hash invariant of on-disk blobs is preserved.
    pub fn write(&self, kind: &str, content: &[u8]) -> Result<Hash, StoreError> {
        let hash = Hash::of_content(content);
        if find_blob_path(&self.kinora_root, &hash).is_some() {
            return Ok(hash);
        }
        let ext = ext_for_kind(kind);
        let path = store_blob_path_with_ext(&self.kinora_root, &hash, ext);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_name = match ext {
            Some(e) => format!(".tmp-{}.{e}", hash.as_hex()),
            None => format!(".tmp-{}", hash.as_hex()),
        };
        let tmp = path.parent().expect("shard dir").join(tmp_name);
        fs::write(&tmp, content)?;
        fs::rename(&tmp, &path)?;
        Ok(hash)
    }

    pub fn read(&self, hash: &Hash) -> Result<Vec<u8>, StoreError> {
        let path = find_blob_path(&self.kinora_root, hash).ok_or_else(|| {
            StoreError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no blob for hash {}", hash.as_hex()),
            ))
        })?;
        let bytes = fs::read(&path)?;
        let actual = Hash::of_content(&bytes);
        if &actual != hash {
            return Err(StoreError::HashMismatch {
                expected: hash.clone(),
                got: actual,
                path,
            });
        }
        Ok(bytes)
    }

    pub fn exists(&self, hash: &Hash) -> bool {
        find_blob_path(&self.kinora_root, hash).is_some()
    }

    pub fn ensure_layout(&self) -> Result<(), StoreError> {
        fs::create_dir_all(store_dir(&self.kinora_root))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn store() -> (TempDir, ContentStore) {
        let tmp = TempDir::new().unwrap();
        let store = ContentStore::new(tmp.path().to_path_buf());
        store.ensure_layout().unwrap();
        (tmp, store)
    }

    #[test]
    fn write_then_read_roundtrips_bytes() {
        let (_tmp, store) = store();
        let content = b"kinora test content";
        let hash = store.write("markdown", content).unwrap();
        let out = store.read(&hash).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn write_is_sharded_by_first_two_hex() {
        let (_tmp, store) = store();
        let hash = store.write("markdown", b"hello").unwrap();
        let expected_dir = store.root().join("store").join(hash.shard());
        assert!(expected_dir.is_dir(), "shard dir missing: {}", expected_dir.display());
        let expected_file = expected_dir.join(format!("{}.md", hash.as_hex()));
        assert!(expected_file.is_file(), "blob file missing: {}", expected_file.display());
    }

    #[test]
    fn write_is_idempotent() {
        let (_tmp, store) = store();
        let h1 = store.write("markdown", b"same").unwrap();
        let h2 = store.write("markdown", b"same").unwrap();
        assert_eq!(h1, h2);
        assert!(store.exists(&h1));
    }

    #[test]
    fn exists_returns_false_for_absent() {
        let (_tmp, store) = store();
        let absent: Hash = "00".repeat(32).parse().unwrap();
        assert!(!store.exists(&absent));
    }

    #[test]
    fn read_verifies_hash_and_detects_corruption() {
        let (_tmp, store) = store();
        let hash = store.write("markdown", b"authentic").unwrap();
        let path = find_blob_path(store.root(), &hash).unwrap();
        fs::write(&path, b"tampered").unwrap();
        let err = store.read(&hash).unwrap_err();
        assert!(matches!(err, StoreError::HashMismatch { .. }));
    }

    #[test]
    fn content_is_pure_no_injected_metadata() {
        let (_tmp, store) = store();
        let content = b"exact bytes";
        let hash = store.write("markdown", content).unwrap();
        let path = find_blob_path(store.root(), &hash).unwrap();
        let raw = fs::read(&path).unwrap();
        assert_eq!(raw, content);
    }

    #[test]
    fn large_content_roundtrips() {
        let (_tmp, store) = store();
        let content = vec![0xABu8; 10_000];
        let hash = store.write("markdown", &content).unwrap();
        let out = store.read(&hash).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn write_uses_kind_derived_extension() {
        let (_tmp, store) = store();
        for (kind, ext) in [("markdown", Some("md")), ("text", Some("txt")), ("kinograph", Some("styx"))] {
            let content = format!("content-for-{kind}").into_bytes();
            let hash = store.write(kind, &content).unwrap();
            let path = find_blob_path(store.root(), &hash).unwrap();
            let filename = path.file_name().unwrap().to_string_lossy().into_owned();
            let expected = match ext {
                Some(e) => format!("{}.{e}", hash.as_hex()),
                None => hash.as_hex().to_owned(),
            };
            assert_eq!(filename, expected, "kind {kind} produced unexpected filename");
        }
    }

    #[test]
    fn write_with_binary_kind_has_no_extension() {
        let (_tmp, store) = store();
        let hash = store.write("binary", b"opaque").unwrap();
        let path = find_blob_path(store.root(), &hash).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            hash.as_hex()
        );
    }

    #[test]
    fn write_with_namespaced_kind_falls_back_to_bin_extension() {
        let (_tmp, store) = store();
        let hash = store.write("team::sketch", b"weird").unwrap();
        let path = find_blob_path(store.root(), &hash).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}.bin", hash.as_hex())
        );
    }

    #[test]
    fn same_content_different_kind_dedupes_to_first_writer_extension() {
        let (_tmp, store) = store();
        let bytes = b"identical content";
        let h1 = store.write("markdown", bytes).unwrap();
        let h2 = store.write("text", bytes).unwrap();
        assert_eq!(h1, h2);
        let path = find_blob_path(store.root(), &h1).unwrap();
        // First writer (markdown) wins the extension; the text-kind call
        // deduped to the existing file.
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}.md", h1.as_hex())
        );
    }

    #[test]
    fn read_finds_extensionless_legacy_blob() {
        // Simulate a blob written by an older kinora that did not append
        // extensions. `read` must still locate it by scanning the shard dir.
        let (_tmp, store) = store();
        let bytes = b"legacy blob";
        let hash = Hash::of_content(bytes);
        let legacy_path = store.root().join("store").join(hash.shard()).join(hash.as_hex());
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(&legacy_path, bytes).unwrap();
        let out = store.read(&hash).unwrap();
        assert_eq!(out, bytes);
    }

    #[test]
    fn find_blob_path_ignores_stale_tmp_from_a_crashed_write() {
        // A crashed write leaves `.tmp-<hash>.<ext>` in the shard dir. The
        // stem-is-hash invariant means readers should not treat this as a
        // real blob — `find_blob_path` splits on the first dot, so a tmp
        // name whose stem is `<hash>` would falsely match.
        let (_tmp, store) = store();
        let hash = Hash::of_content(b"imminent");
        let shard = store.root().join("store").join(hash.shard());
        fs::create_dir_all(&shard).unwrap();
        let stale = shard.join(format!(".tmp-{}.md", hash.as_hex()));
        fs::write(&stale, b"partial").unwrap();
        assert!(
            find_blob_path(store.root(), &hash).is_none(),
            "stale tmp file leaked through as a blob"
        );
        assert!(!store.exists(&hash));
    }

    #[test]
    fn read_errors_when_blob_absent() {
        let (_tmp, store) = store();
        let missing: Hash = "ab".repeat(32).parse().unwrap();
        let err = store.read(&missing).unwrap_err();
        assert!(matches!(err, StoreError::Io(_)));
    }
}

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use sha2::{Digest, Sha256};

use crate::StorageError;

/// Reference to a content-addressed blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobRef {
    pub sha256: String,
    pub byte_len: u64,
}

/// Result of verifying a blob's integrity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobIntegrity {
    Valid,
    Missing,
    SizeMismatch { expected: u64, actual: u64 },
    HashMismatch { expected: String, actual: String },
}

/// Trait for content-addressed blob storage.
pub trait BlobStore: Send + Sync {
    fn store(&self, data: &[u8]) -> Result<BlobRef, StorageError>;
    fn read(&self, blob_ref: &BlobRef) -> Result<Vec<u8>, StorageError>;
    fn exists(&self, blob_ref: &BlobRef) -> Result<bool, StorageError>;
    fn verify(&self, blob_ref: &BlobRef) -> Result<BlobIntegrity, StorageError>;
    fn blob_path(&self, blob_ref: &BlobRef) -> PathBuf;
}

/// Compute SHA-256 hex digest for the given bytes.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// LocalBlobStore
// ---------------------------------------------------------------------------

/// Content-addressed blob store backed by the local filesystem.
///
/// Layout: `{root}/blobs/sha256/{first_2}/{next_2}/{full_hash}.blob`
///
/// Write path: hash -> write to temp file in `blobs/.tmp/` -> fsync -> atomic
/// rename to final location. If the final path already exists, the rename is
/// skipped (content-addressed = deduplication is safe).
#[derive(Clone, Debug)]
pub struct LocalBlobStore {
    root: PathBuf,
}

impl LocalBlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        let tmp_dir = root.join("blobs").join(".tmp");
        fs::create_dir_all(&tmp_dir).map_err(|e| {
            StorageError::Internal(format!(
                "failed to create blob tmp dir {}: {e}",
                tmp_dir.display()
            ))
        })?;
        tracing::info!(blob_root = %root.display(), "opened local blob store");
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn fanout_path(&self, sha256: &str) -> PathBuf {
        let first2 = &sha256[..2];
        let next2 = &sha256[2..4];
        self.root
            .join("blobs")
            .join("sha256")
            .join(first2)
            .join(next2)
            .join(format!("{sha256}.blob"))
    }
}

impl BlobStore for LocalBlobStore {
    fn store(&self, data: &[u8]) -> Result<BlobRef, StorageError> {
        let hash = sha256_hex(data);
        let byte_len = data.len() as u64;
        let final_path = self.fanout_path(&hash);

        // Deduplication: if blob already exists, skip write.
        if final_path.exists() {
            tracing::debug!(sha256 = %hash, byte_len, deduplicated = true, "blob already exists");
            return Ok(BlobRef {
                sha256: hash,
                byte_len,
            });
        }

        // Ensure parent directory exists.
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to create blob directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        // Write to temp file, fsync, then atomic rename.
        let tmp_dir = self.root.join("blobs").join(".tmp");
        let tmp_path = tmp_dir.join(format!("{hash}.tmp"));

        let mut file = fs::File::create(&tmp_path).map_err(|e| {
            StorageError::Internal(format!(
                "failed to create temp blob file {}: {e}",
                tmp_path.display()
            ))
        })?;

        file.write_all(data)
            .map_err(|e| StorageError::Internal(format!("failed to write blob data: {e}")))?;

        file.sync_all()
            .map_err(|e| StorageError::Internal(format!("failed to fsync blob file: {e}")))?;

        drop(file);

        // Atomic rename. On failure due to existing target, that is fine (race).
        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => {}
            Err(e) if final_path.exists() => {
                // Another writer raced and won. Clean up our temp.
                let _ = fs::remove_file(&tmp_path);
                tracing::debug!(sha256 = %hash, "blob appeared during write (race), dedup");
                return Ok(BlobRef {
                    sha256: hash,
                    byte_len,
                });
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(StorageError::Internal(format!(
                    "failed to rename blob to {}: {e}",
                    final_path.display()
                )));
            }
        }

        tracing::debug!(sha256 = %hash, byte_len, deduplicated = false, "blob stored");

        Ok(BlobRef {
            sha256: hash,
            byte_len,
        })
    }

    fn read(&self, blob_ref: &BlobRef) -> Result<Vec<u8>, StorageError> {
        let path = self.fanout_path(&blob_ref.sha256);
        fs::read(&path).map_err(|e| {
            StorageError::NotFound(format!(
                "blob {} not readable at {}: {e}",
                blob_ref.sha256,
                path.display()
            ))
        })
    }

    fn exists(&self, blob_ref: &BlobRef) -> Result<bool, StorageError> {
        let path = self.fanout_path(&blob_ref.sha256);
        Ok(path.exists())
    }

    fn verify(&self, blob_ref: &BlobRef) -> Result<BlobIntegrity, StorageError> {
        let path = self.fanout_path(&blob_ref.sha256);

        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => return Ok(BlobIntegrity::Missing),
        };

        if !metadata.is_file() {
            return Ok(BlobIntegrity::Missing);
        }

        let actual_size = metadata.len();
        if actual_size != blob_ref.byte_len {
            return Ok(BlobIntegrity::SizeMismatch {
                expected: blob_ref.byte_len,
                actual: actual_size,
            });
        }

        let data = fs::read(&path).map_err(|e| {
            StorageError::Internal(format!("failed to read blob for verification: {e}"))
        })?;

        let actual_hash = sha256_hex(&data);
        if actual_hash != blob_ref.sha256 {
            return Ok(BlobIntegrity::HashMismatch {
                expected: blob_ref.sha256.clone(),
                actual: actual_hash,
            });
        }

        Ok(BlobIntegrity::Valid)
    }

    fn blob_path(&self, blob_ref: &BlobRef) -> PathBuf {
        self.fanout_path(&blob_ref.sha256)
    }
}

// ---------------------------------------------------------------------------
// InMemoryBlobStore
// ---------------------------------------------------------------------------

/// In-memory blob store for tests.
#[derive(Clone, Default)]
pub struct InMemoryBlobStore {
    blobs: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl InMemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlobStore for InMemoryBlobStore {
    fn store(&self, data: &[u8]) -> Result<BlobRef, StorageError> {
        let hash = sha256_hex(data);
        let byte_len = data.len() as u64;
        let mut blobs = self
            .blobs
            .write()
            .map_err(|_| StorageError::Internal("failed to lock in-memory blob store".into()))?;
        blobs.entry(hash.clone()).or_insert_with(|| data.to_vec());
        Ok(BlobRef {
            sha256: hash,
            byte_len,
        })
    }

    fn read(&self, blob_ref: &BlobRef) -> Result<Vec<u8>, StorageError> {
        let blobs = self
            .blobs
            .read()
            .map_err(|_| StorageError::Internal("failed to lock in-memory blob store".into()))?;
        blobs
            .get(&blob_ref.sha256)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(format!("blob {} not found", blob_ref.sha256)))
    }

    fn exists(&self, blob_ref: &BlobRef) -> Result<bool, StorageError> {
        let blobs = self
            .blobs
            .read()
            .map_err(|_| StorageError::Internal("failed to lock in-memory blob store".into()))?;
        Ok(blobs.contains_key(&blob_ref.sha256))
    }

    fn verify(&self, blob_ref: &BlobRef) -> Result<BlobIntegrity, StorageError> {
        let blobs = self
            .blobs
            .read()
            .map_err(|_| StorageError::Internal("failed to lock in-memory blob store".into()))?;
        match blobs.get(&blob_ref.sha256) {
            None => Ok(BlobIntegrity::Missing),
            Some(data) => {
                let actual_size = data.len() as u64;
                if actual_size != blob_ref.byte_len {
                    return Ok(BlobIntegrity::SizeMismatch {
                        expected: blob_ref.byte_len,
                        actual: actual_size,
                    });
                }
                let actual_hash = sha256_hex(data);
                if actual_hash != blob_ref.sha256 {
                    return Ok(BlobIntegrity::HashMismatch {
                        expected: blob_ref.sha256.clone(),
                        actual: actual_hash,
                    });
                }
                Ok(BlobIntegrity::Valid)
            }
        }
    }

    fn blob_path(&self, blob_ref: &BlobRef) -> PathBuf {
        PathBuf::from(format!("memory://blob/{}", blob_ref.sha256))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_blob_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("replaykit-blob-tests")
            .join(name)
            .join(format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn local_write_read_round_trip() {
        let root = temp_blob_root("round-trip");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"hello, blob store!";
        let blob_ref = store.store(data).unwrap();

        assert_eq!(blob_ref.byte_len, data.len() as u64);
        assert_eq!(blob_ref.sha256, sha256_hex(data));

        let read_back = store.read(&blob_ref).unwrap();
        assert_eq!(read_back, data);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_deduplication() {
        let root = temp_blob_root("dedup");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"deduplicate me";
        let ref1 = store.store(data).unwrap();
        let ref2 = store.store(data).unwrap();

        assert_eq!(ref1, ref2);
        assert_eq!(store.blob_path(&ref1), store.blob_path(&ref2));

        // Only one file on disk.
        let path = store.blob_path(&ref1);
        assert!(path.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_fanout_path_derivation() {
        let root = temp_blob_root("fanout");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"fanout test";
        let hash = sha256_hex(data);
        let first2 = &hash[..2];
        let next2 = &hash[2..4];

        let blob_ref = BlobRef {
            sha256: hash.clone(),
            byte_len: data.len() as u64,
        };
        let path = store.blob_path(&blob_ref);

        let expected = root
            .join("blobs")
            .join("sha256")
            .join(first2)
            .join(next2)
            .join(format!("{hash}.blob"));
        assert_eq!(path, expected);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_verify_valid() {
        let root = temp_blob_root("verify-valid");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"verify me";
        let blob_ref = store.store(data).unwrap();
        assert_eq!(store.verify(&blob_ref).unwrap(), BlobIntegrity::Valid);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_verify_missing() {
        let root = temp_blob_root("verify-missing");
        let store = LocalBlobStore::open(&root).unwrap();

        let blob_ref = BlobRef {
            sha256: "a".repeat(64),
            byte_len: 10,
        };
        assert_eq!(store.verify(&blob_ref).unwrap(), BlobIntegrity::Missing);
        assert!(!store.exists(&blob_ref).unwrap());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_verify_size_mismatch() {
        let root = temp_blob_root("verify-size");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"size check";
        let blob_ref = store.store(data).unwrap();

        // Claim different size.
        let wrong_ref = BlobRef {
            sha256: blob_ref.sha256.clone(),
            byte_len: blob_ref.byte_len + 999,
        };
        match store.verify(&wrong_ref).unwrap() {
            BlobIntegrity::SizeMismatch { expected, actual } => {
                assert_eq!(expected, blob_ref.byte_len + 999);
                assert_eq!(actual, blob_ref.byte_len);
            }
            other => panic!("expected SizeMismatch, got {:?}", other),
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_verify_hash_mismatch_after_corruption() {
        let root = temp_blob_root("verify-hash");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"corrupt me";
        let blob_ref = store.store(data).unwrap();

        // Corrupt the file (overwrite with same size but different content).
        let path = store.blob_path(&blob_ref);
        fs::write(&path, b"corraptxme").unwrap();

        match store.verify(&blob_ref).unwrap() {
            BlobIntegrity::HashMismatch { expected, actual } => {
                assert_eq!(expected, blob_ref.sha256);
                assert_ne!(actual, expected);
            }
            other => panic!("expected HashMismatch, got {:?}", other),
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_empty_content() {
        let root = temp_blob_root("empty");
        let store = LocalBlobStore::open(&root).unwrap();

        let data = b"";
        let blob_ref = store.store(data).unwrap();
        assert_eq!(blob_ref.byte_len, 0);

        let read_back = store.read(&blob_ref).unwrap();
        assert!(read_back.is_empty());
        assert_eq!(store.verify(&blob_ref).unwrap(), BlobIntegrity::Valid);

        let _ = fs::remove_dir_all(&root);
    }

    // InMemoryBlobStore tests

    #[test]
    fn memory_write_read_round_trip() {
        let store = InMemoryBlobStore::new();

        let data = b"in-memory blob";
        let blob_ref = store.store(data).unwrap();
        let read_back = store.read(&blob_ref).unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn memory_deduplication() {
        let store = InMemoryBlobStore::new();

        let data = b"dedup in memory";
        let ref1 = store.store(data).unwrap();
        let ref2 = store.store(data).unwrap();
        assert_eq!(ref1, ref2);
    }

    #[test]
    fn memory_verify_missing() {
        let store = InMemoryBlobStore::new();

        let blob_ref = BlobRef {
            sha256: "b".repeat(64),
            byte_len: 5,
        };
        assert_eq!(store.verify(&blob_ref).unwrap(), BlobIntegrity::Missing);
    }
}

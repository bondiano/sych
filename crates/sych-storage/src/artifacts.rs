//! Local content-addressed artifact directory. Bytes are immutable: a key is
//! the SHA-256 of the bytes, and every read verifies it.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use sych_core::{Sha256Digest, sha256};

use crate::{Partition, StorageError};

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<partition hash>/<aa>/<hex>`; the partition keeps tenants and scopes apart on disk.
    pub fn key(partition: &Partition, digest: &Sha256Digest) -> String {
        let part =
            sha256(format!("{}\0{}", partition.tenant_id, partition.access_scope).as_bytes());
        let hex = digest.hex();
        format!("{}/{}/{hex}", &part.hex()[..32], &hex[..2])
    }

    fn path(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    /// Writes and syncs the bytes before any pointer may reference them.
    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.path(key);
        if let Ok(existing) = fs::read(&path) {
            return if existing == bytes {
                Ok(())
            } else {
                Err(StorageError::Corrupt(format!(
                    "artifact {key} differs from its digest"
                )))
            };
        }

        let dir = path.parent().unwrap_or(&self.root);
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::now_v7()));
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &path)?;
        File::open(dir)?.sync_all()?;
        Ok(())
    }

    /// Reads and verifies; a missing file is reported unavailable, never regenerated.
    pub fn get(&self, key: &str, digest: &Sha256Digest) -> Result<Vec<u8>, StorageError> {
        let bytes = match fs::read(self.path(key)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(StorageError::ArtifactUnavailable(digest.clone()));
            }
            Err(err) => return Err(err.into()),
        };
        if &sha256(&bytes) == digest {
            Ok(bytes)
        } else {
            Err(StorageError::Corrupt(format!(
                "artifact {digest} fails its hash check"
            )))
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        match fs::remove_file(self.path(key)) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

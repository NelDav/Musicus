//! Content-addressed file store: `blobs/ab/abcdef…` named by SHA-256.
//!
//! Files are written to `tmp/` while hashing, synced, then renamed into
//! place, so a blob path either does not exist or holds complete content.
//! Blobs are never modified; identical uploads share one file.

use axum::body::Body;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct BlobStore {
    root: PathBuf,
}

/// A fully written, hashed upload that is not yet in the store.
pub struct PendingBlob {
    pub tmp_path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug)]
pub enum UploadError {
    TooLarge(u64),
    Io(io::Error),
    Body(String),
}

impl From<io::Error> for UploadError {
    fn from(e: io::Error) -> Self {
        UploadError::Io(e)
    }
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BlobStore {
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let store = Self { root: data_dir.to_owned() };
        std::fs::create_dir_all(store.root.join("blobs"))?;
        std::fs::create_dir_all(store.tmp_dir())?;
        // Leftovers of interrupted uploads.
        for entry in std::fs::read_dir(store.tmp_dir())? {
            let _ = std::fs::remove_file(entry?.path());
        }
        Ok(store)
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Path of a blob; `sha256` must be lowercase hex.
    pub fn path(&self, sha256: &str) -> PathBuf {
        debug_assert!(is_sha256_hex(sha256));
        self.root.join("blobs").join(&sha256[..2]).join(sha256)
    }

    /// Streams a request body to a temporary file, hashing it on the way.
    pub async fn receive(&self, body: Body, limit: u64) -> Result<PendingBlob, UploadError> {
        let (token, _) = crate::auth::new_token();
        let tmp_path = self.tmp_dir().join(&token[..32]);
        let result = async {
            let mut file = tokio::fs::File::create(&tmp_path).await?;
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| UploadError::Body(e.to_string()))?;
                size += chunk.len() as u64;
                if size > limit {
                    return Err(UploadError::TooLarge(limit));
                }
                hasher.update(&chunk);
                file.write_all(&chunk).await?;
            }
            file.sync_all().await?;
            Ok((hex::encode(hasher.finalize()), size))
        }
        .await;
        match result {
            Ok((sha256, size)) => Ok(PendingBlob { tmp_path, sha256, size }),
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                Err(e)
            }
        }
    }

    /// Moves a pending upload into the store (or drops it if the content
    /// already exists).
    pub async fn commit(&self, pending: &PendingBlob) -> io::Result<()> {
        let target = self.path(&pending.sha256);
        if tokio::fs::try_exists(&target).await? {
            tokio::fs::remove_file(&pending.tmp_path).await?;
            return Ok(());
        }
        let dir = target.parent().expect("blob paths have a parent");
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::rename(&pending.tmp_path, &target).await?;
        sync_dir(dir).await
    }

    pub async fn discard(&self, pending: &PendingBlob) {
        let _ = tokio::fs::remove_file(&pending.tmp_path).await;
    }

    /// Stores small derived content (e.g. thumbnails) and returns its hash.
    pub async fn put_bytes(&self, bytes: &[u8]) -> io::Result<String> {
        let sha256 = hex::encode(Sha256::digest(bytes));
        let target = self.path(&sha256);
        if tokio::fs::try_exists(&target).await? {
            return Ok(sha256);
        }
        let (token, _) = crate::auth::new_token();
        let tmp_path = self.tmp_dir().join(&token[..32]);
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        let dir = target.parent().expect("blob paths have a parent");
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::rename(&tmp_path, &target).await?;
        sync_dir(dir).await?;
        Ok(sha256)
    }
}

async fn sync_dir(dir: &Path) -> io::Result<()> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || std::fs::File::open(dir)?.sync_all()).await.map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_by_hash_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let a = store.receive(Body::from("hello"), 100).await.unwrap();
        assert_eq!(a.sha256, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        store.commit(&a).await.unwrap();
        let b = store.receive(Body::from("hello"), 100).await.unwrap();
        store.commit(&b).await.unwrap();
        assert_eq!(std::fs::read(store.path(&a.sha256)).unwrap(), b"hello");
        assert_eq!(std::fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn rejects_oversized_bodies_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        assert!(matches!(store.receive(Body::from("0123456789"), 5).await, Err(UploadError::TooLarge(5))));
        assert_eq!(std::fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    }
}

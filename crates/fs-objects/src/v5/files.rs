//! Where received and written files live (fs-files at the border): a small trait and a memory implementation.
//!
//! Pattern: Adapter port. A `FileRef` carries the checksum, so a read can be checked against what was promised.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use fs_types::{BasicValue, FileRef, MediaType, Reference};

use super::codec::invalid;
use super::error::{Error, Result};
use super::original::{hex32, sha256_hex};

/// Read and write files by reference.
#[async_trait]
pub trait FileStore: Send + Sync {
    /// The bytes of a file.
    async fn read(&self, f: &FileRef) -> Result<Vec<u8>>;
    /// Keep bytes; the reference carries their checksum.
    async fn write(&self, bytes: Vec<u8>, media: &MediaType) -> Result<FileRef>;
}

/// Files in memory (tests, and programs before fs-files is wired in).
#[derive(Default)]
pub struct MemoryFiles(Mutex<HashMap<String, Vec<u8>>>);

impl MemoryFiles {
    /// Put bytes in and get the reference (`urn:sha256:<hex>`).
    ///
    /// # Errors
    /// `Unavailable` when the reference cannot be built (never in practice: it is made from hex).
    pub fn put(&self, bytes: &[u8], media: &MediaType) -> Result<FileRef> {
        let hex = sha256_hex(bytes);
        let mut sha = [0_u8; 32];
        for (i, b) in sha.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap_or(0);
        }
        let iri = Reference::parse(&format!("urn:sha256:{hex}"))
            .map_err(|_| Error::Unavailable("no reference for a checksum".into()))?;
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(iri.canonical(), bytes.to_vec());
        Ok(FileRef {
            iri,
            sha256: sha,
            media_type: media.clone(),
        })
    }
}

#[async_trait]
impl FileStore for MemoryFiles {
    async fn read(&self, f: &FileRef) -> Result<Vec<u8>> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&f.iri.canonical())
            .cloned();
        let bytes = bytes
            .ok_or_else(|| Error::Unavailable(format!("no such file {}", f.iri.canonical())))?;
        if sha256_hex(&bytes) != hex32(&f.sha256) {
            return Err(invalid(
                "source",
                "the checksum of the file differs from its reference",
            ));
        }
        Ok(bytes)
    }

    async fn write(&self, bytes: Vec<u8>, media: &MediaType) -> Result<FileRef> {
        self.put(&bytes, media)
    }
}

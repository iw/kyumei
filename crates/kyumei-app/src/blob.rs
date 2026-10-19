//! Content-addressed storage for binary evidence (screenshots, large files).
//!
//! Blobs are written once under their SHA-256 and never modified, so the
//! evidence record's hash always identifies exactly what was submitted.

use std::{collections::BTreeMap, fs, io, path::PathBuf, sync::Mutex};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::core::{Command, StoredContent};

/// Submission content larger than this is moved into the blob store before
/// it is sent to a durable workflow, which then carries only its hash.
pub const EXTERNALIZE_ABOVE: usize = 64 * 1024;

#[derive(Debug)]
pub struct BlobStore {
    backend: Backend,
}

#[derive(Debug)]
enum Backend {
    Memory(Mutex<BTreeMap<String, Vec<u8>>>),
    Directory(PathBuf),
}

impl BlobStore {
    pub fn memory() -> Self {
        Self {
            backend: Backend::Memory(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn directory(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self {
            backend: Backend::Directory(dir),
        })
    }

    fn valid(sha: &str) -> io::Result<()> {
        if sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidInput, "not a SHA-256"))
        }
    }

    pub fn put(&self, sha: &str, bytes: &[u8]) -> io::Result<()> {
        Self::valid(sha)?;
        match &self.backend {
            Backend::Memory(m) => {
                m.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(sha.to_owned())
                    .or_insert_with(|| bytes.to_vec());
                Ok(())
            }
            Backend::Directory(dir) => {
                let path = dir.join(sha);
                if path.exists() {
                    return Ok(());
                }
                let tmp = dir.join(format!("{sha}.tmp"));
                fs::write(&tmp, bytes)?;
                fs::rename(tmp, path)
            }
        }
    }

    pub fn get(&self, sha: &str) -> io::Result<Option<Vec<u8>>> {
        Self::valid(sha)?;
        match &self.backend {
            Backend::Memory(m) => Ok(m
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(sha)
                .cloned()),
            Backend::Directory(dir) => match fs::read(dir.join(sha)) {
                Ok(b) => Ok(Some(b)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
        }
    }
}

/// Move a large submission's content into `store`, leaving a reference in
/// the command. Small submissions, and other commands, are left as they are.
pub fn externalize(command: &mut Command, store: &BlobStore) -> io::Result<()> {
    let Command::SubmitEvidence(sub) = command else {
        return Ok(());
    };
    if sub.stored.is_some() {
        return Ok(());
    }
    let (bytes, text) = match (&sub.bytes_base64, &sub.text) {
        (Some(b64), _) if b64.len() > EXTERNALIZE_ABOVE => match STANDARD.decode(b64) {
            Ok(bytes) => (bytes, false),
            // Left for the core to reject with a proper message.
            Err(_) => return Ok(()),
        },
        (None, Some(t)) if t.len() > EXTERNALIZE_ABOVE => (t.as_bytes().to_vec(), true),
        _ => return Ok(()),
    };
    let sha256 = kyumei_evidence::sha256_hex(&bytes);
    store.put(&sha256, &bytes)?;
    sub.stored = Some(StoredContent {
        sha256,
        text,
        size_bytes: bytes.len() as u64,
    });
    sub.text = None;
    sub.bytes_base64 = None;
    Ok(())
}

/// Put content that [`externalize`] moved into `store` back into the
/// command. A reference to content the store does not hold is `NotFound`.
pub fn resolve(command: &mut Command, store: &BlobStore) -> io::Result<()> {
    let Command::SubmitEvidence(sub) = command else {
        return Ok(());
    };
    let Some(stored) = sub.stored.take() else {
        return Ok(());
    };
    let bytes = store.get(&stored.sha256)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "submitted content {} is not in the blob store",
                stored.sha256
            ),
        )
    })?;
    if stored.text {
        sub.text = Some(
            String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
        );
    } else {
        sub.bytes_base64 = Some(STANDARD.encode(bytes));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use kyumei_domain::SubmissionChannel;

    use super::*;
    use crate::core::EvidenceSubmission;

    #[test]
    fn large_submissions_travel_by_reference() {
        let store = BlobStore::memory();
        let text = "x".repeat(EXTERNALIZE_ABOVE + 1);
        let original = Command::SubmitEvidence(EvidenceSubmission::text(
            SubmissionChannel::Paste,
            text.clone(),
        ));
        let mut command = original.clone();
        externalize(&mut command, &store).expect("externalize");
        let Command::SubmitEvidence(sub) = &command else {
            unreachable!()
        };
        assert!(sub.text.is_none() && sub.bytes_base64.is_none());
        let stored = sub.stored.clone().expect("a reference");
        assert_eq!(stored.size_bytes, text.len() as u64);
        assert!(serde_json::to_vec(&command).expect("json").len() < 1024);

        resolve(&mut command, &store).expect("resolve");
        assert_eq!(command, original);

        let mut missing = Command::SubmitEvidence(EvidenceSubmission {
            stored: Some(StoredContent {
                sha256: "0".repeat(64),
                ..stored
            }),
            text: None,
            ..EvidenceSubmission::text(SubmissionChannel::Paste, "")
        });
        let err = resolve(&mut missing, &store).expect_err("missing content");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn small_submissions_stay_inline() {
        let store = BlobStore::memory();
        let original = Command::SubmitEvidence(EvidenceSubmission::text(
            SubmissionChannel::Paste,
            "2026-09-30T13:35:00Z 180",
        ));
        let mut command = original.clone();
        externalize(&mut command, &store).expect("externalize");
        assert_eq!(command, original);
    }
}

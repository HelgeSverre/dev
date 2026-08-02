use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_MANIFEST_SIZE: u64 = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ManifestCache {
    values: Mutex<HashMap<PathBuf, Result<String, String>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest `{path}` is too large ({size} bytes)")]
    TooLarge { path: PathBuf, size: u64 },
    #[error("failed to read manifest `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cached manifest read failed for `{path}`: {message}")]
    Cached { path: PathBuf, message: String },
    #[error("manifest cache lock is poisoned")]
    Poisoned,
}

impl ManifestCache {
    pub fn read(&self, path: &Path) -> Result<String, ManifestError> {
        if let Some(cached) = self
            .values
            .lock()
            .map_err(|_| ManifestError::Poisoned)?
            .get(path)
            .cloned()
        {
            return cached.map_err(|message| ManifestError::Cached {
                path: path.to_path_buf(),
                message,
            });
        }

        let value = read_bounded(path).map_err(|error| error.to_string());
        self.values
            .lock()
            .map_err(|_| ManifestError::Poisoned)?
            .insert(path.to_path_buf(), value.clone());
        value.map_err(|message| ManifestError::Cached {
            path: path.to_path_buf(),
            message,
        })
    }
}

fn read_bounded(path: &Path) -> Result<String, ManifestError> {
    let metadata = path.metadata().map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_MANIFEST_SIZE {
        return Err(ManifestError::TooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
        });
    }
    std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })
}

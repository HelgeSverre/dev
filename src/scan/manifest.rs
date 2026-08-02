use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_MANIFEST_SIZE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct DiscoveryFiles {
    values: Arc<Mutex<HashMap<PathBuf, Result<String, String>>>>,
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

impl DiscoveryFiles {
    pub fn read(&self, path: &Path) -> Result<String, ManifestError> {
        if let Some(Ok(cached)) = self
            .values
            .lock()
            .map_err(|_| ManifestError::Poisoned)?
            .get(path)
            .cloned()
        {
            return Ok(cached);
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

    pub fn read_paths(&self) -> Result<Vec<PathBuf>, ManifestError> {
        let mut paths = self
            .values
            .lock()
            .map_err(|_| ManifestError::Poisoned)?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }
}

fn read_bounded(path: &Path) -> Result<String, ManifestError> {
    let file = std::fs::File::open(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_MANIFEST_SIZE {
        return Err(ManifestError::TooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
        });
    }
    let mut contents = String::new();
    file.take(MAX_MANIFEST_SIZE + 1)
        .read_to_string(&mut contents)
        .map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if contents.len() as u64 > MAX_MANIFEST_SIZE {
        return Err(ManifestError::TooLarge {
            path: path.to_path_buf(),
            size: contents.len() as u64,
        });
    }
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_reads_are_retried_without_losing_shape_tracking() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let manifest = temporary.path().join("wibblewabble.toml");
        let files = DiscoveryFiles::default();

        assert!(files.read(&manifest).is_err());
        assert_eq!(
            files.read_paths()?.as_slice(),
            std::slice::from_ref(&manifest)
        );

        std::fs::write(&manifest, "answer = 42\n")?;
        assert_eq!(files.read(&manifest)?, "answer = 42\n");
        assert_eq!(files.read_paths()?, [manifest]);
        Ok(())
    }

    #[test]
    fn oversized_manifests_are_rejected() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let manifest = temporary.path().join("large.json");
        std::fs::write(&manifest, vec![b'x'; MAX_MANIFEST_SIZE as usize + 1])?;

        assert!(matches!(
            read_bounded(&manifest),
            Err(ManifestError::TooLarge { .. })
        ));
        Ok(())
    }
}

use std::fs::OpenOptions;
use std::io::{self, BufReader, Read as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest as _, Sha256};

use crate::schema::{Artifact, ArtifactKind};
use crate::{Error, Result};

pub const MAX_RUNTIME_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_MODEL_DOWNLOAD_BYTES: u64 = 400 * 1024 * 1024;
pub const MAX_OTHER_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

pub trait Downloader: Send + Sync {
    fn download(&self, artifact: &Artifact, destination: &Path, cancel: &AtomicBool) -> Result<()>;

    fn download_with_progress(
        &self,
        artifact: &Artifact,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<()> {
        self.download(artifact, destination, cancel)?;
        progress(artifact.byte_count, artifact.byte_count);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct HttpDownloader {
    timeout: Duration,
}

impl Default for HttpDownloader {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15 * 60),
        }
    }
}

impl HttpDownloader {
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Downloader for HttpDownloader {
    fn download(&self, artifact: &Artifact, destination: &Path, cancel: &AtomicBool) -> Result<()> {
        self.download_impl(artifact, destination, cancel, &|_, _| {})
    }

    fn download_with_progress(
        &self,
        artifact: &Artifact,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<()> {
        self.download_impl(artifact, destination, cancel, progress)
    }
}

impl HttpDownloader {
    fn download_impl(
        &self,
        artifact: &Artifact,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<()> {
        let configured_limit = match artifact.kind {
            ArtifactKind::Runtime => MAX_RUNTIME_DOWNLOAD_BYTES,
            ArtifactKind::Model => MAX_MODEL_DOWNLOAD_BYTES,
            ArtifactKind::Other => MAX_OTHER_DOWNLOAD_BYTES,
        };
        if artifact.byte_count > configured_limit {
            return Err(Error::DownloadTooLarge {
                limit: configured_limit,
            });
        }
        if !artifact.url.starts_with("https://") {
            return Err(Error::Download {
                url: artifact.url.clone(),
                message: "only HTTPS downloads are permitted".into(),
            });
        }
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(5)
            .max_redirects_will_error(true)
            .timeout_global(Some(self.timeout))
            .build();
        let agent: ureq::Agent = config.into();
        let mut response = agent
            .get(&artifact.url)
            .header(
                "User-Agent",
                concat!("UCI-Grabber/", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .map_err(|source| Error::Download {
                url: artifact.url.clone(),
                message: source.to_string(),
            })?;
        if let Some(length) = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            && length != artifact.byte_count
        {
            return Err(Error::Download {
                url: artifact.url.clone(),
                message: format!(
                    "server reported {length} bytes, recipe declares {}",
                    artifact.byte_count
                ),
            });
        }
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(|source| Error::io(destination, source))?;
        let mut writer = io::BufWriter::new(file);
        let mut reader = response.body_mut().as_reader();
        let mut digest = Sha256::new();
        let mut total = 0_u64;
        let mut last_reported = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let read = reader.read(&mut buffer).map_err(|source| Error::Download {
                url: artifact.url.clone(),
                message: source.to_string(),
            })?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            if total > artifact.byte_count || total > configured_limit {
                return Err(Error::DownloadTooLarge {
                    limit: artifact.byte_count.min(configured_limit),
                });
            }
            digest.update(&buffer[..read]);
            writer
                .write_all(&buffer[..read])
                .map_err(|source| Error::io(destination, source))?;
            if total == artifact.byte_count || total.saturating_sub(last_reported) >= 512 * 1024 {
                progress(total, artifact.byte_count);
                last_reported = total;
            }
        }
        writer
            .flush()
            .map_err(|source| Error::io(destination, source))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| Error::io(destination, source))?;
        if total != artifact.byte_count {
            return Err(Error::Download {
                url: artifact.url.clone(),
                message: format!(
                    "received {total} bytes, recipe declares {}",
                    artifact.byte_count
                ),
            });
        }
        let actual = format!("{:x}", digest.finalize());
        if !actual.eq_ignore_ascii_case(&artifact.sha256) {
            return Err(Error::ChecksumMismatch {
                path: destination.to_path_buf(),
                expected: artifact.sha256.clone(),
                actual,
            });
        }
        Ok(())
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|source| Error::io(path, source))?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| Error::io(path, source))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

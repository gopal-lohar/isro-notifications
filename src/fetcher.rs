use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tracing::warn;

/// A downloaded page plus its checksum.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub body: Bytes,
    pub checksum: String,
}

impl Snapshot {
    pub fn new(body: Bytes) -> Self {
        let mut checksum = String::with_capacity(64);
        for byte in Sha256::digest(&body) {
            let _ = write!(checksum, "{byte:02x}");
        }
        Self { body, checksum }
    }
}

#[derive(Clone, Debug)]
pub enum FetchEvent {
    /// First successful fetch: cache primed, nothing to report.
    Baseline { checksum: String },
    /// Same bytes as the cached snapshot.
    Unchanged { checksum: String },
    /// Bytes differ from the cached snapshot.
    Changed {
        previous: Snapshot,
        current: Snapshot,
    },
    /// Download or validation failed; the cache is left untouched.
    Failed { error: String },
}

/// Polls one URL and keeps the last usable response in memory.
pub struct Fetcher {
    client: Client,
    url: String,
    validate: fn(&[u8]) -> Result<(), String>,
    state: Option<PathBuf>,
    last: Option<Snapshot>,
}

impl Fetcher {
    pub fn new(
        client: Client,
        url: impl Into<String>,
        validate: fn(&[u8]) -> Result<(), String>,
    ) -> Self {
        Self {
            client,
            url: url.into(),
            validate,
            state: None,
            last: None,
        }
    }

    /// Load the last snapshot from disk, so a restart behaves like a normal poll
    /// instead of a fresh baseline.
    pub fn with_state(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Ok(body) = std::fs::read(&path) {
            if (self.validate)(&body).is_ok() {
                self.last = Some(Snapshot::new(body.into()));
            } else {
                warn!(path = %path.display(), "ignoring invalid snapshot state");
            }
        }
        self.state = Some(path);
        self
    }

    pub fn last(&self) -> Option<&Snapshot> {
        self.last.as_ref()
    }

    /// Fetch once, compare against the cached snapshot and update it.
    pub async fn poll(&mut self) -> FetchEvent {
        let body = match self.download().await {
            Ok(body) => body,
            Err(error) => return FetchEvent::Failed { error },
        };
        if let Err(error) = (self.validate)(&body) {
            return FetchEvent::Failed { error };
        }

        let current = Snapshot::new(body);
        let event = match self.last.take() {
            None => FetchEvent::Baseline {
                checksum: current.checksum.clone(),
            },
            Some(previous) if previous.checksum == current.checksum => FetchEvent::Unchanged {
                checksum: current.checksum.clone(),
            },
            Some(previous) => FetchEvent::Changed {
                previous,
                current: current.clone(),
            },
        };
        self.last = Some(current.clone());

        if let Some(path) = &self.state
            && let Err(error) = save(path, &current.body)
        {
            warn!(path = %path.display(), %error, "failed to persist snapshot");
        }
        event
    }

    async fn download(&self) -> Result<Bytes, String> {
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        response
            .bytes()
            .await
            .map_err(|e| format!("body read failed: {e}"))
    }
}

fn save(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

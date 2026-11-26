use std::error::Error;
use std::path::{Path, PathBuf};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use reqwest::Client;

/// Shared HTTP client for the whole process (connection pooling, default headers).
pub fn client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/html,application/xhtml+xml"));
    Client::builder()
        .user_agent("Mozilla/5.0")
        .default_headers(headers)
        .build()
        .expect("reqwest client")
}

/// Fetch `url` into `dir` as `isro-<timestamp>.html` and return the path.
pub async fn fetch(
    client: &Client,
    url: &str,
    dir: &Path,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let path = dir.join(format!(
        "isro-{}.html",
        chrono::Local::now().format("%Y-%m-%d-%H-%M-%S")
    ));
    std::fs::write(&path, body)?;
    Ok(path)
}

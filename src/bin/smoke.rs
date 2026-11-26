use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use isro_notifications::analytics::Analytics;
use std::time::Duration;

use isro_notifications::config::{self, Config};
use isro_notifications::diff;
use isro_notifications::enrich::{self, Summarizer};
use isro_notifications::fetcher::{FetchEvent, Fetcher};
use isro_notifications::pipeline::Pipeline;
use isro_notifications::sarvam::Sarvam;
use isro_notifications::scrapper;
use isro_notifications::telegram::{Telegram, TelegramConfig};
use isro_notifications::web::{self, WebState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{error, info};

const CHECKS: usize = 6;
const INTERVAL: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    init_tracing();

    let a = read("isro-2026-08-29-17-06-58.html");
    let b = read("isro-2026-09-04-22-01-32.html");
    let c = read("isro-2026-09-06-16-42-25.html");
    // baseline, update, no change, no change, update, no change
    let sequence = vec![a, b.clone(), b.clone(), b, c.clone(), c];
    let addr = serve(sequence).await;

    let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| config::DEFAULT_PATH.into());
    let mut config = Config::load(Path::new(&path))?;
    config.isro.url = format!("http://{addr}/");
    config.isro.base_url = "https://www.isro.gov.in".into();
    if let Ok(api) = std::env::var("TELEGRAM_API") {
        config.telegram.api = api;
    }
    if let Ok(registry) = std::env::var("TELEGRAM_REGISTRY") {
        config.telegram.registry = registry.into();
    }

    let client = scrapper::client();
    let sarvam_key = std::env::var("SARVAM_API_KEY").map_err(|_| "SARVAM_API_KEY not set")?;
    let token = std::env::var("TELEGRAM_BOT_TOKEN").map_err(|_| "TELEGRAM_BOT_TOKEN not set")?;

    let mut telegram_config = TelegramConfig::new(token, &config.telegram.registry);
    telegram_config.api = config.telegram.api.clone();
    telegram_config.admin_username = config.telegram.admin_username.clone();
    telegram_config.welcome = config.telegram.welcome.clone();
    telegram_config.concurrency = config.telegram.concurrency;
    let telegram = Telegram::new(client.clone(), telegram_config);

    let added = telegram.poll_updates().await?;
    if added > 0 {
        info!(added, "registered new chats");
    }
    if telegram.active_chats() == 0 {
        return Err("no active chats: send /start to the bot, then run again".into());
    }

    let analytics = Arc::new(Mutex::new(Analytics::new()));
    let pipeline = Pipeline::new(
        client.clone(),
        enrich::Config {
            max_chars: config.summary.max_chars,
            base_url: config.isro.base_url.clone(),
            concurrency: config.summary.concurrency,
        },
        config.telegram.alerts.clone(),
        analytics.clone(),
        Some(Box::new(Sarvam::new(client.clone(), sarvam_key)) as Box<dyn Summarizer>),
        Box::new(telegram.clone()),
    );

    let bind = std::env::var("ANALYTICS_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    if !bind.trim().is_empty() {
        match TcpListener::bind(bind.as_str()).await {
            Ok(listener) => {
                let addr = listener
                    .local_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| bind.clone());
                let state = WebState {
                    analytics: analytics.clone(),
                    telegram: Some(telegram),
                    admin_username: config.telegram.admin_username.clone(),
                };
                tokio::spawn(async move {
                    if let Err(error) = web::serve(listener, state).await {
                        error!(%error, "analytics server stopped");
                    }
                });
                info!(%addr, "analytics server listening");
            }
            Err(error) => error!(%error, %bind, "analytics bind failed"),
        }
    }

    let mut fetcher = Fetcher::new(client, config.isro.url.clone(), diff::validate);
    let mut outcomes = Vec::new();
    for check in 0..CHECKS {
        let event = fetcher.poll().await;
        let label = label(&event);
        let updates = pipeline.handle(event).await;
        info!(check, label, updates = updates.len(), "check");
        outcomes.push((label, updates.len()));
        if check + 1 < CHECKS {
            tokio::time::sleep(INTERVAL).await;
        }
    }

    let expected = [
        ("baseline", 0),
        ("changed", 2),
        ("unchanged", 0),
        ("unchanged", 0),
        ("changed", 2),
        ("unchanged", 0),
    ];
    if outcomes != expected {
        return Err(format!("unexpected sequence: {outcomes:?}, expected {expected:?}").into());
    }

    info!("smoke test passed");
    Ok(())
}

fn label(event: &FetchEvent) -> &'static str {
    match event {
        FetchEvent::Baseline { .. } => "baseline",
        FetchEvent::Unchanged { .. } => "unchanged",
        FetchEvent::Changed { .. } => "changed",
        FetchEvent::Failed { .. } => "failed",
    }
}

fn read(name: &str) -> Vec<u8> {
    let path = format!("{}/.dev/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// Serves the snapshots in order, one per request, repeating the last one.
async fn serve(sequence: Vec<Vec<u8>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let index = Arc::new(Mutex::new(0usize));
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let sequence = sequence.clone();
            let index = index.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let i = {
                    let mut next = index.lock().unwrap();
                    let i = *next;
                    *next = (*next + 1).min(sequence.len() - 1);
                    i
                };
                let body = &sequence[i];
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}

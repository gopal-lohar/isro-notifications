use std::time::Duration;

use isro_notifications::diff;
use isro_notifications::enrich::{self, Summarizer};
use isro_notifications::fetcher::Fetcher;
use isro_notifications::pipeline::{LogDeliverer, Pipeline};
use isro_notifications::sarvam::Sarvam;
use isro_notifications::scrapper;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    init_tracing();

    let url = std::env::var("ISRO_URL").unwrap_or_else(|_| "https://www.isro.gov.in".into());
    let base_url = std::env::var("ISRO_BASE_URL").unwrap_or_else(|_| url.clone());
    let interval = Duration::from_secs(
        std::env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(30 * 60),
    );

    let client = scrapper::client();
    let summarizer = std::env::var("SARVAM_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .map(|key| Box::new(Sarvam::new(client.clone(), key)) as Box<dyn Summarizer>);
    let has_summarizer = summarizer.is_some();

    let pipeline = Pipeline::new(
        client.clone(),
        enrich::Config {
            max_chars: env_parse("SUMMARY_MAX_CHARS", 1024),
            base_url,
        },
        summarizer,
        Box::new(LogDeliverer),
    );

    info!(
        %url,
        interval_secs = interval.as_secs(),
        summarizer = has_summarizer,
        "starting isro-notifications"
    );

    let mut fetcher = Fetcher::new(client, url, diff::validate);
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                pipeline.handle(fetcher.poll().await).await;
            }
            _ = shutdown() => {
                info!("shutdown signal received");
                break;
            }
        }
    }

    Ok(())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

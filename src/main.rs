use std::time::Duration;

use isro_notifications::diff::{self, DiffEvent};
use isro_notifications::enrich::{self, Summarizer};
use isro_notifications::fetcher::{FetchEvent, Fetcher};
use isro_notifications::sarvam::Sarvam;
use isro_notifications::scrapper;
use reqwest::Client;
use tracing::{debug, error, info};

struct App {
    client: Client,
    enrich: enrich::Config,
    summarizer: Option<Box<dyn Summarizer>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    init_tracing();

    let url = std::env::var("ISRO_URL").unwrap_or_else(|_| "https://www.isro.gov.in".into());
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

    let app = App {
        client: client.clone(),
        enrich: enrich::Config {
            max_chars: env_parse("SUMMARY_MAX_CHARS", 1024),
            base_url: url.clone(),
        },
        summarizer,
    };

    info!(
        %url,
        interval_secs = interval.as_secs(),
        summarizer = app.summarizer.is_some(),
        "starting isro-notifications"
    );

    let mut fetcher = Fetcher::new(client, url, diff::validate);
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = tick.tick() => poll_once(&mut fetcher, &app).await,
            _ = shutdown() => {
                info!("shutdown signal received");
                break;
            }
        }
    }

    Ok(())
}

async fn poll_once(fetcher: &mut Fetcher, app: &App) {
    match fetcher.poll().await {
        FetchEvent::Baseline { checksum } => info!(%checksum, "baseline snapshot stored"),
        FetchEvent::Unchanged { checksum } => debug!(%checksum, "no change"),
        FetchEvent::Changed { previous, current } => {
            debug!(from = %previous.checksum, to = %current.checksum, "page changed");
            match diff::compare(&previous.body, &current.body, &app.enrich.base_url) {
                DiffEvent::Failure { message } => error!(%message, "site failure"),
                DiffEvent::NoNews => info!("page changed but no new news"),
                DiffEvent::News { items } => {
                    let updates =
                        enrich::enrich(&app.client, &items, &app.enrich, app.summarizer.as_deref())
                            .await;
                    for update in &updates {
                        info!("{}", update);
                    }
                }
            }
        }
        FetchEvent::Failed { error } => error!(%error, "fetch failed"),
    }
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

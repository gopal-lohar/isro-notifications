use std::time::Duration;

use isro_notifications::diff::{self, DiffEvent};
use isro_notifications::fetcher::{FetchEvent, Fetcher};
use isro_notifications::scrapper;
use tracing::{debug, error, info};

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

    info!(%url, interval_secs = interval.as_secs(), "starting isro-notifications");

    let mut fetcher = Fetcher::new(scrapper::client(), url.clone(), diff::validate);
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = tick.tick() => poll_once(&mut fetcher, &url).await,
            _ = shutdown() => {
                info!("shutdown signal received");
                break;
            }
        }
    }

    Ok(())
}

async fn poll_once(fetcher: &mut Fetcher, base_url: &str) {
    match fetcher.poll().await {
        FetchEvent::Baseline { checksum } => info!(%checksum, "baseline snapshot stored"),
        FetchEvent::Unchanged { checksum } => debug!(%checksum, "no change"),
        FetchEvent::Changed { previous, current } => {
            debug!(from = %previous.checksum, to = %current.checksum, "page changed");
            match diff::compare(&previous.body, &current.body, base_url) {
                DiffEvent::Failure { message } => error!(%message, "site failure"),
                DiffEvent::NoNews => info!("page changed but no new news"),
                DiffEvent::News { items } => {
                    for item in &items {
                        info!("{}", item);
                    }
                }
            }
        }
        FetchEvent::Failed { error } => error!(%error, "fetch failed"),
    }
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

use std::path::Path;
use std::time::Duration;

use isro_notifications::config::{self, Config};
use isro_notifications::diff;
use isro_notifications::enrich::{self, Summarizer};
use isro_notifications::fetcher::Fetcher;
use isro_notifications::pipeline::{Deliverer, LogDeliverer, Pipeline};
use isro_notifications::sarvam::Sarvam;
use isro_notifications::scrapper;
use isro_notifications::telegram::{Telegram, TelegramConfig};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    init_tracing();

    let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| config::DEFAULT_PATH.into());
    let config = Config::load(Path::new(&path))?;

    let client = scrapper::client();
    let summarizer = std::env::var("SARVAM_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .map(|key| Box::new(Sarvam::new(client.clone(), key)) as Box<dyn Summarizer>);
    let has_summarizer = summarizer.is_some();

    let telegram = std::env::var("TELEGRAM_BOT_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .map(|token| {
            let mut telegram = TelegramConfig::new(token, &config.telegram.registry);
            telegram.api = config.telegram.api.clone();
            telegram.admin_username = config.telegram.admin_username.clone();
            telegram.welcome = config.telegram.welcome.clone();
            Telegram::new(client.clone(), telegram)
        });

    let deliverer: Box<dyn Deliverer> = match &telegram {
        Some(telegram) => Box::new(telegram.clone()),
        None => Box::new(LogDeliverer),
    };

    let pipeline = Pipeline::new(
        client.clone(),
        enrich::Config {
            max_chars: config.summary.max_chars,
            base_url: config.isro.base_url.clone(),
        },
        config.telegram.alerts.clone(),
        summarizer,
        deliverer,
    );

    if let Some(telegram) = telegram.clone() {
        tokio::spawn(async move {
            loop {
                match telegram.poll_updates().await {
                    Ok(0) => {}
                    Ok(added) => info!(added, "registered new telegram chats"),
                    Err(error) => warn!(%error, "telegram getUpdates failed"),
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    info!(
        url = %config.isro.url,
        interval_secs = config.isro.poll_interval_secs,
        summarizer = has_summarizer,
        telegram = telegram.is_some(),
        "starting isro-notifications"
    );

    let mut fetcher = Fetcher::new(client, config.isro.url.clone(), diff::validate);
    let mut tick = tokio::time::interval(Duration::from_secs(
        config.isro.poll_interval_secs.max(1),
    ));
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

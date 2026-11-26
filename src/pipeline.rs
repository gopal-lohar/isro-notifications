use std::future::Future;
use std::pin::Pin;

use reqwest::Client;
use tracing::{debug, error, info};

use crate::config::Alerts;
use crate::diff::{self, DiffEvent};
use crate::enrich::{self, Summarizer, Update};
use crate::fetcher::FetchEvent;

/// Where finished updates go. Telegram is one implementation; logging is another.
pub trait Deliverer: Send + Sync {
    fn deliver<'a>(
        &'a self,
        update: &'a Update,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Send a failure alert to the operator. Default: no-op.
    fn alert<'a>(
        &'a self,
        _message: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

/// Logs updates to the tracing output.
pub struct LogDeliverer;

impl Deliverer for LogDeliverer {
    fn deliver<'a>(
        &'a self,
        update: &'a Update,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            info!("{}", update);
            Ok(())
        })
    }
}

/// fetch event -> diff -> enrich -> deliver.
pub struct Pipeline {
    client: Client,
    enrich: enrich::Config,
    alerts: Alerts,
    summarizer: Option<Box<dyn Summarizer>>,
    deliverer: Box<dyn Deliverer>,
}

impl Pipeline {
    pub fn new(
        client: Client,
        enrich: enrich::Config,
        alerts: Alerts,
        summarizer: Option<Box<dyn Summarizer>>,
        deliverer: Box<dyn Deliverer>,
    ) -> Self {
        Self {
            client,
            enrich,
            alerts,
            summarizer,
            deliverer,
        }
    }

    async fn alert(&self, message: &str) {
        if let Err(error) = self.deliverer.alert(message).await {
            error!(%error, "alert delivery failed");
        }
    }

    /// Handle one poll result. Returns the updates that were produced.
    pub async fn handle(&self, event: FetchEvent) -> Vec<Update> {
        match event {
            FetchEvent::Baseline { checksum } => {
                info!(%checksum, "baseline snapshot stored");
                Vec::new()
            }
            FetchEvent::Unchanged { checksum } => {
                debug!(%checksum, "no change");
                Vec::new()
            }
            FetchEvent::Failed { error } => {
                error!(%error, "fetch failed");
                self.alert(&self.alerts.fetch_failed.replace("{error}", &error))
                    .await;
                Vec::new()
            }
            FetchEvent::Changed { previous, current } => {
                debug!(from = %previous.checksum, to = %current.checksum, "page changed");
                match diff::compare(&previous.body, &current.body, &self.enrich.base_url) {
                    DiffEvent::Failure { message } => {
                        error!(%message, "site failure");
                        self.alert(&self.alerts.site_failure.replace("{message}", &message))
                            .await;
                        Vec::new()
                    }
                    DiffEvent::NoNews => {
                        info!("page changed but no new news");
                        Vec::new()
                    }
                    DiffEvent::News { items } => {
                        let updates = enrich::enrich(
                            &self.client,
                            &items,
                            &self.enrich,
                            self.summarizer.as_deref(),
                        )
                        .await;
                        for update in &updates {
                            if let Err(error) = self.deliverer.deliver(update).await {
                                error!(%error, "delivery failed");
                            }
                        }
                        updates
                    }
                }
            }
        }
    }
}

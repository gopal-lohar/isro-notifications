use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::diff::Kind;
use crate::enrich::Update;

const RECENT_LIMIT: usize = 50;

#[derive(Clone, Debug, Serialize)]
pub struct RecentUpdate {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub title: String,
    pub link: Option<String>,
    pub attachments: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub started_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub status: String,
    pub updates_sent: u64,
    pub recent: Vec<RecentUpdate>,
}

#[derive(Debug)]
pub struct Analytics {
    started_at: DateTime<Utc>,
    last_run: Option<DateTime<Utc>>,
    status: String,
    updates_sent: u64,
    recent: VecDeque<RecentUpdate>,
}

impl Default for Analytics {
    fn default() -> Self {
        Self::new()
    }
}

impl Analytics {
    pub fn new() -> Self {
        Self {
            started_at: Utc::now(),
            last_run: None,
            status: "starting".to_string(),
            updates_sent: 0,
            recent: VecDeque::new(),
        }
    }

    pub fn record_run(&mut self, status: impl Into<String>) {
        self.last_run = Some(Utc::now());
        self.status = status.into();
    }

    pub fn record_update(&mut self, update: &Update) {
        self.updates_sent += 1;
        self.recent.push_front(RecentUpdate {
            at: Utc::now(),
            kind: match update.kind {
                Kind::Flash => "Flash",
                Kind::Latest => "Latest",
            }
            .to_string(),
            title: update
                .text
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
            link: update.link.clone(),
            attachments: update.attachments.len(),
        });
        self.recent.truncate(RECENT_LIMIT);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            started_at: self.started_at,
            last_run: self.last_run,
            status: self.status.clone(),
            updates_sent: self.updates_sent,
            recent: self.recent.iter().cloned().collect(),
        }
    }
}

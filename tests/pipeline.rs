use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use isro_notifications::config::Alerts;
use isro_notifications::enrich::{Config, Summarizer, Update};
use isro_notifications::fetcher::{FetchEvent, Snapshot};
use isro_notifications::pipeline::{Deliverer, Pipeline};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Recording {
    delivered: Arc<Mutex<Vec<Update>>>,
    alerts: Arc<Mutex<Vec<String>>>,
    fail: bool,
}

impl Recording {
    fn new(fail: bool) -> Self {
        Self {
            delivered: Arc::new(Mutex::new(Vec::new())),
            alerts: Arc::new(Mutex::new(Vec::new())),
            fail,
        }
    }

    fn delivered(&self) -> Vec<Update> {
        self.delivered.lock().unwrap().clone()
    }

    fn alerts(&self) -> Vec<String> {
        self.alerts.lock().unwrap().clone()
    }
}

impl Deliverer for Recording {
    fn deliver<'a>(
        &'a self,
        update: &'a Update,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.delivered.lock().unwrap().push(update.clone());
            if self.fail {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        })
    }

    fn alert<'a>(
        &'a self,
        message: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.alerts.lock().unwrap().push(message.to_string());
            Ok(())
        })
    }
}

struct Fake(&'static str);

impl Summarizer for Fake {
    fn summarize<'a>(
        &'a self,
        _title: &'a str,
        _text: &'a str,
        _max_chars: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move { Ok(self.0.to_string()) })
    }
}

fn pipeline(rec: &Recording, summarizer: Option<Box<dyn Summarizer>>, base_url: &str) -> Pipeline {
    Pipeline::new(
        reqwest::Client::new(),
        Config {
            max_chars: 1024,
            base_url: base_url.to_string(),
            concurrency: 4,
        },
        Alerts::default(),
        summarizer,
        Box::new(rec.clone()),
    )
}

/// Minimal flash-news page; padded past the 512-byte sanity floor.
fn page(items: &[(&str, &str)]) -> Vec<u8> {
    let flash: String = items
        .iter()
        .map(|(title, link)| {
            format!(
                r#"<p class="flashContent"><b class="flash">Flash News:</b><span class="out" title="{title}"><a href="{link}">{title}</a></span></p>"#
            )
        })
        .collect();
    format!(
        "<html><body>{flash}<!-- {} --></body></html>",
        "x".repeat(600)
    )
    .into_bytes()
}

fn changed(old: Vec<u8>, new: Vec<u8>) -> FetchEvent {
    FetchEvent::Changed {
        previous: Snapshot::new(old.into()),
        current: Snapshot::new(new.into()),
    }
}

async fn serve(body: String) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn baseline_unchanged_and_failed_deliver_nothing() {
    let rec = Recording::new(false);
    let pipeline = pipeline(&rec, None, "https://www.isro.gov.in");

    assert!(
        pipeline
            .handle(FetchEvent::Baseline {
                checksum: "a".into()
            })
            .await
            .is_empty()
    );
    assert!(
        pipeline
            .handle(FetchEvent::Unchanged {
                checksum: "a".into()
            })
            .await
            .is_empty()
    );
    assert!(
        pipeline
            .handle(FetchEvent::Failed {
                error: "boom".into()
            })
            .await
            .is_empty()
    );
    assert!(rec.delivered().is_empty());
}

#[tokio::test]
async fn failures_are_alerted() {
    let rec = Recording::new(false);
    let pipeline = pipeline(&rec, None, "https://www.isro.gov.in");

    pipeline
        .handle(FetchEvent::Failed {
            error: "HTTP 503".into(),
        })
        .await;
    assert_eq!(rec.alerts(), vec!["fetch failed: HTTP 503".to_string()]);

    let blocked = b"<html><body>Access Denied</body></html>".to_vec();
    pipeline
        .handle(changed(page(&[("A", "https://example.com/a")]), blocked))
        .await;

    let alerts = rec.alerts();
    assert_eq!(alerts.len(), 2);
    assert!(alerts[1].starts_with("site failure:"), "{:?}", alerts[1]);
    assert!(rec.delivered().is_empty());
}

#[tokio::test]
async fn changed_page_delivers_new_items() {
    let rec = Recording::new(false);
    let pipeline = pipeline(&rec, None, "https://www.isro.gov.in");

    let updates = pipeline
        .handle(changed(
            page(&[("A", "https://example.com/a")]),
            page(&[
                ("A", "https://example.com/a"),
                ("B", "https://example.com/b"),
            ]),
        ))
        .await;

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].text, "B");
    assert_eq!(updates[0].link.as_deref(), Some("https://example.com/b"));
    assert_eq!(rec.delivered().len(), 1);
}

#[tokio::test]
async fn changed_page_without_additions_delivers_nothing() {
    let rec = Recording::new(false);
    let pipeline = pipeline(&rec, None, "https://www.isro.gov.in");

    let updates = pipeline
        .handle(changed(
            page(&[("A", "https://example.com/a")]),
            page(&[("A", "https://example.com/a")]),
        ))
        .await;

    assert!(updates.is_empty());
    assert!(rec.delivered().is_empty());
}

#[tokio::test]
async fn delivery_failure_does_not_stop_other_updates() {
    let rec = Recording::new(true);
    let pipeline = pipeline(&rec, None, "https://www.isro.gov.in");

    let updates = pipeline
        .handle(changed(
            page(&[("A", "https://example.com/a")]),
            page(&[
                ("A", "https://example.com/a"),
                ("B", "https://example.com/b"),
                ("C", "https://example.com/c"),
            ]),
        ))
        .await;

    assert_eq!(updates.len(), 2);
    assert_eq!(rec.delivered().len(), 2);
}

#[tokio::test]
async fn internal_link_is_summarised_and_delivered() {
    let article = r#"<html><body>
        <h1 class="pageHeading"><span>Article title</span></h1>
        <p class="pageContent">Body text.</p>
        <img src="/media/pic.webp" alt="pic">
    </body></html>"#;
    let addr = serve(article.to_string()).await;
    let base = format!("http://{addr}");
    let link = format!("{base}/article.html");

    let rec = Recording::new(false);
    let pipeline = pipeline(&rec, Some(Box::new(Fake("Title\n\nSummary."))), &base);

    let updates = pipeline
        .handle(changed(
            page(&[("A", "https://example.com/a")]),
            page(&[("A", "https://example.com/a"), ("B", &link)]),
        ))
        .await;

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].text, "B\n\nSummary.");
    assert_eq!(updates[0].attachments.len(), 1);
    assert_eq!(updates[0].attachments[0].url, format!("{base}/media/pic.webp"));
    assert_eq!(rec.delivered().len(), 1);
}

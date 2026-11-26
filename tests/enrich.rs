use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use isro_notifications::diff::Kind;
use isro_notifications::enrich::{self, AttachmentKind, Config, Summarizer};
use isro_notifications::sarvam::Sarvam;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Routes = Arc<Mutex<HashMap<String, (u16, String)>>>;
type LastRequest = Arc<Mutex<Option<Captured>>>;

#[derive(Clone, Default)]
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Minimal HTTP/1.1 server: serves `routes` by path and records the last request.
async fn serve(routes: Routes, last: LastRequest) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let routes = routes.clone();
            let last = last.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head_end = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|i| i + 4)
                    .unwrap_or(buf.len());
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let mut request = lines.next().unwrap_or_default().split_whitespace();
                let method = request.next().unwrap_or_default().to_string();
                let path = request.next().unwrap_or_default().to_string();

                let mut headers = Vec::new();
                let mut content_length = 0;
                for line in lines {
                    if let Some((key, value)) = line.split_once(':') {
                        let key = key.trim().to_lowercase();
                        let value = value.trim().to_string();
                        if key == "content-length" {
                            content_length = value.parse().unwrap_or(0);
                        }
                        headers.push((key, value));
                    }
                }

                let mut body = buf[head_end..].to_vec();
                while body.len() < content_length {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }

                *last.lock().unwrap() = Some(Captured {
                    method,
                    path: path.clone(),
                    headers,
                    body: String::from_utf8_lossy(&body).to_string(),
                });

                let (status, response) = routes
                    .lock()
                    .unwrap()
                    .get(&path)
                    .cloned()
                    .unwrap_or((404, "not found".to_string()));
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

fn article(name: &str) -> String {
    let path = format!("{}/.dev/articles/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

struct Fake {
    calls: AtomicUsize,
    reply: String,
}

impl Fake {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            reply: reply.into(),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Summarizer for Fake {
    fn summarize<'a>(
        &'a self,
        _title: &'a str,
        _text: &'a str,
        _max_chars: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.reply.clone())
        })
    }
}

struct Failing;

impl Summarizer for Failing {
    fn summarize<'a>(
        &'a self,
        _title: &'a str,
        _text: &'a str,
        _max_chars: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async { Err("boom".to_string()) })
    }
}

fn item(link: &str) -> isro_notifications::diff::NewsItem {
    titled_item("Original title", link)
}

fn titled_item(title: &str, link: &str) -> isro_notifications::diff::NewsItem {
    isro_notifications::diff::NewsItem {
        kind: Kind::Latest,
        title: title.to_string(),
        link: Some(link.to_string()),
    }
}

#[tokio::test]
async fn external_link_passes_through_without_fetching() {
    let routes: Routes = Arc::new(Mutex::new(HashMap::new()));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last.clone()).await;
    let cfg = Config::new(format!("http://{addr}"));
    let fake = Fake::new("should not be used");

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item("https://example.com/story.html")],
        &cfg,
        Some(&fake),
    )
    .await;

    assert_eq!(updates[0].text, "Original title");
    assert!(updates[0].attachments.is_empty());
    assert_eq!(fake.calls(), 0);
    assert!(last.lock().unwrap().is_none());
}

#[tokio::test]
async fn internal_link_without_summarizer_passes_through() {
    let routes: Routes = Arc::new(Mutex::new(HashMap::new()));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last.clone()).await;
    let cfg = Config::new(format!("http://{addr}"));

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item(&format!("http://{addr}/story.html"))],
        &cfg,
        None,
    )
    .await;

    assert_eq!(updates[0].text, "Original title");
    assert!(last.lock().unwrap().is_none());
}

#[tokio::test]
async fn internal_link_is_summarised_with_attachments() {
    let name = "Ground_test_of_third_stage_of_PSLV_PS3.html";
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        format!("/{name}"),
        (200, article(name)),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));
    let fake = Fake::new("PS3 ground test succeeds\n\nISRO conducted a ground static test of the PSLV third stage.");

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item(&format!("http://{addr}/{name}"))],
        &cfg,
        Some(&fake),
    )
    .await;

    assert_eq!(fake.calls(), 1);
    assert_eq!(
        updates[0].text,
        "Original title\n\nISRO conducted a ground static test of the PSLV third stage."
    );
    assert_eq!(updates[0].attachments.len(), 2);
    assert!(
        updates[0]
            .attachments
            .iter()
            .all(|a| a.kind == AttachmentKind::Image)
    );
    assert!(
        updates[0].attachments[0]
            .url
            .starts_with(&format!("http://{addr}/media_isro/image/index/Latest/"))
    );
}

#[tokio::test]
async fn cached_articles_parse_into_text_and_media() {
    let cases = [
        (
            "Mission_GSLVF18.html",
            "GSLV-F18 has successfully placed NVS-03 in the intended orbit.",
        ),
        (
            "Thirtieth_Session_of_the_Intergovernmental_Consultative_Committee.html",
            "ISRO’s Participation in the Thirtieth Session of the Intergovernmental Consultative Committee (ICC-30)",
        ),
        (
            "Ground_test_of_third_stage_of_PSLV_PS3.html",
            "Ground test of third stage of PSLV (PS3) conducted successfully",
        ),
    ];
    let routes: Routes = Arc::new(Mutex::new(
        cases
            .iter()
            .map(|(name, _)| (format!("/{name}"), (200, article(name))))
            .collect(),
    ));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));
    let fake = Fake::new("Title\n\nSummary.");

    for (name, _) in cases {
        let updates = enrich::enrich(
            &reqwest::Client::new(),
            &[item(&format!("http://{addr}/{name}"))],
            &cfg,
            Some(&fake),
        )
        .await;

        assert_eq!(updates[0].text, "Original title\n\nSummary.", "{name}");
        assert!(!updates[0].attachments.is_empty(), "{name} has no media");
        assert!(
            updates[0]
                .attachments
                .iter()
                .all(|a| a.url.starts_with(&format!("http://{addr}/media_isro/"))),
            "{name} has an unresolved attachment"
        );
    }
}

#[tokio::test]
async fn summary_without_blank_line_is_reformatted() {
    let name = "Ground_test_of_third_stage_of_PSLV_PS3.html";
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        format!("/{name}"),
        (200, article(name)),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));
    let fake = Fake::new("Title line\nSummary without a blank line.");

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item(&format!("http://{addr}/{name}"))],
        &cfg,
        Some(&fake),
    )
    .await;

    assert_eq!(
        updates[0].text,
        "Original title\n\nSummary without a blank line."
    );
}

#[tokio::test]
async fn passthrough_title_shares_the_budget_with_the_link() {
    let cfg = Config {
        max_chars: 100,
        base_url: "https://www.isro.gov.in".to_string(),
        concurrency: 4,
    };
    let link = "https://example.com/a-link-that-eats-some-budget.html";
    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[titled_item(&"x".repeat(200), link)],
        &cfg,
        None,
    )
    .await;

    let total = updates[0].text.chars().count() + 2 + link.chars().count();
    assert!(total <= cfg.max_chars, "total {total} exceeds budget");
}

#[tokio::test]
async fn title_is_enforced_when_the_model_rewrites_it() {
    let name = "Ground_test_of_third_stage_of_PSLV_PS3.html";
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        format!("/{name}"),
        (200, article(name)),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));
    let fake = Fake::new("Rewritten title\n\nSummary.");

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[titled_item("Original title", &format!("http://{addr}/{name}"))],
        &cfg,
        Some(&fake),
    )
    .await;

    assert_eq!(updates[0].text, "Original title\n\nSummary.");
}

#[tokio::test]
async fn meta_lines_are_stripped() {
    let name = "Ground_test_of_third_stage_of_PSLV_PS3.html";
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        format!("/{name}"),
        (200, article(name)),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));

    for reply in [
        "Title\n\nSummary.\n\nCharacter count: 694",
        "Title\n\nSummary.\n\n(119 characters)",
    ] {
        let fake = Fake::new(reply);
        let updates = enrich::enrich(
            &reqwest::Client::new(),
            &[titled_item("Original title", &format!("http://{addr}/{name}"))],
            &cfg,
            Some(&fake),
        )
        .await;

        assert_eq!(updates[0].text, "Original title\n\nSummary.");
    }
}

#[tokio::test]
async fn long_summary_is_retried_then_trimmed() {
    let name = "Ground_test_of_third_stage_of_PSLV_PS3.html";
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        format!("/{name}"),
        (200, article(name)),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config {
        max_chars: 100,
        base_url: format!("http://{addr}"),
        concurrency: 4,
    };
    let fake = Fake::new("x".repeat(2000));

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item(&format!("http://{addr}/{name}"))],
        &cfg,
        Some(&fake),
    )
    .await;

    assert_eq!(fake.calls(), 2);
    let link = updates[0].link.as_deref().unwrap();
    let total = updates[0].text.chars().count() + 2 + link.chars().count();
    assert!(total <= cfg.max_chars, "total {total} exceeds budget");
    assert!(!updates[0].text.is_empty());
}

#[tokio::test]
async fn summarizer_error_falls_back_to_link() {
    let name = "Ground_test_of_third_stage_of_PSLV_PS3.html";
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        format!("/{name}"),
        (200, article(name)),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item(&format!("http://{addr}/{name}"))],
        &cfg,
        Some(&Failing),
    )
    .await;

    assert_eq!(updates[0].text, "Original title");
    assert!(updates[0].attachments.is_empty());
}

#[tokio::test]
async fn article_fetch_failure_falls_back_to_link() {
    let routes: Routes = Arc::new(Mutex::new(HashMap::new()));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));
    let fake = Fake::new("unused");

    let updates = enrich::enrich(
        &reqwest::Client::new(),
        &[item(&format!("http://{addr}/missing.html"))],
        &cfg,
        Some(&fake),
    )
    .await;

    assert_eq!(updates[0].text, "Original title");
    assert_eq!(fake.calls(), 0);
}

#[tokio::test]
#[ignore = "needs SARVAM_API_KEY and network"]
async fn real_sarvam_summary() {
    let Ok(key) = std::env::var("SARVAM_API_KEY") else {
        panic!("SARVAM_API_KEY not set");
    };
    let cases = [
        (
            "Mission_GSLVF18.html",
            "GSLV-F18 has successfully placed NVS-03 in the intended orbit.",
        ),
        (
            "Thirtieth_Session_of_the_Intergovernmental_Consultative_Committee.html",
            "ISRO’s Participation in the Thirtieth Session of the Intergovernmental Consultative Committee (ICC-30)",
        ),
        (
            "Ground_test_of_third_stage_of_PSLV_PS3.html",
            "Ground test of third stage of PSLV (PS3) conducted successfully",
        ),
    ];
    let routes: Routes = Arc::new(Mutex::new(
        cases
            .iter()
            .map(|(name, _)| (format!("/{name}"), (200, article(name))))
            .collect(),
    ));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last).await;
    let cfg = Config::new(format!("http://{addr}"));
    let sarvam = Sarvam::new(reqwest::Client::new(), key);

    for (name, title) in cases {
        let updates = enrich::enrich(
            &reqwest::Client::new(),
            &[titled_item(title, &format!("http://{addr}/{name}"))],
            &cfg,
            Some(&sarvam),
        )
        .await;

        println!("--- {name}\n{}\n", updates[0].text);
        assert_eq!(
            updates[0].text.lines().next(),
            Some(title),
            "{name} title was rewritten"
        );
        let link = updates[0].link.as_deref().unwrap();
        let total = updates[0].text.chars().count() + 2 + link.chars().count();
        assert!(total <= cfg.max_chars, "{name} message too long: {total}");
        assert!(!updates[0].attachments.is_empty(), "{name}");
    }
}

#[tokio::test]
async fn sarvam_client_sends_key_and_model() {
    let reply = r#"{"choices":[{"message":{"content":"Title line\n\nShort summary."}}]}"#;
    let routes: Routes = Arc::new(Mutex::new(HashMap::from([(
        "/v1/chat/completions".to_string(),
        (200, reply.to_string()),
    )])));
    let last: LastRequest = Arc::new(Mutex::new(None));
    let addr = serve(routes, last.clone()).await;

    let sarvam = Sarvam::new(reqwest::Client::new(), "test-key")
        .with_endpoint(format!("http://{addr}/v1/chat/completions"));
    let summary = sarvam
        .summarize("Original title", "Some article text", 1024)
        .await
        .unwrap();

    assert_eq!(summary, "Title line\n\nShort summary.");

    let captured = last.lock().unwrap().clone().expect("request captured");
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.path, "/v1/chat/completions");
    assert!(
        captured
            .headers
            .iter()
            .any(|(k, v)| k == "api-subscription-key" && v == "test-key")
    );
    assert!(captured.body.contains("sarvam-105b"));
    assert!(captured.body.contains("Original title"));
    assert!(captured.body.contains("exactly as given"));
    assert!(captured.body.contains("Some article text"));
    assert!(captured.body.contains("1024 characters"));
}

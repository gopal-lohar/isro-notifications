use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use isro_notifications::diff::Kind;
use isro_notifications::enrich::{Attachment, AttachmentKind, Update};
use isro_notifications::pipeline::Deliverer;
use isro_notifications::registry::{Chat, Registry};
use isro_notifications::telegram::{Telegram, TelegramConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Routes = Arc<Mutex<HashMap<String, (u16, String)>>>;
type Requests = Arc<Mutex<Vec<Captured>>>;

#[derive(Clone, Default)]
struct Captured {
    method: String,
    path: String,
    body: String,
}

fn routes(entries: impl IntoIterator<Item = (&'static str, (u16, &'static str))>) -> Routes {
    Arc::new(Mutex::new(
        entries
            .into_iter()
            .map(|(key, (status, body))| (key.to_string(), (status, body.to_string())))
            .collect(),
    ))
}

/// Minimal Telegram API stand-in, routed by the last path segment.
async fn serve(routes: Routes, last: Requests) -> SocketAddr {
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

                let mut content_length = 0;
                for line in lines {
                    if let Some((key, value)) = line.split_once(':')
                        && key.trim().eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse().unwrap_or(0);
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

                last.lock().unwrap().push(Captured {
                    method,
                    path: path.clone(),
                    body: String::from_utf8_lossy(&body).to_string(),
                });

                let route = path
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let (status, response) = routes
                    .lock()
                    .unwrap()
                    .get(&route)
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

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "isro-telegram-{name}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn write_registry(path: &Path, chats: &[Chat]) {
    Registry {
        offset: 0,
        chats: chats.to_vec(),
    }
    .save(path)
    .unwrap();
}

fn chat(id: i64) -> Chat {
    chat_with(id, "alice")
}

fn chat_with(id: i64, username: &str) -> Chat {
    Chat {
        chat_id: id,
        username: Some(username.to_string()),
        name: username.to_string(),
        active: true,
    }
}

fn telegram(addr: SocketAddr, path: &Path) -> Telegram {
    let mut config = TelegramConfig::new("test-token", path);
    config.api = format!("http://{addr}");
    Telegram::new(reqwest::Client::new(), config)
}

fn update(attachments: Vec<Attachment>) -> Update {
    Update {
        kind: Kind::Latest,
        link: Some("https://www.isro.gov.in/story.html".to_string()),
        text: "Title\n\nSummary.".to_string(),
        attachments,
    }
}

fn image(url: &str) -> Attachment {
    Attachment {
        kind: AttachmentKind::Image,
        url: url.to_string(),
    }
}

#[tokio::test]
async fn poll_updates_registers_chat() {
    let routes = routes([
        (
            "getUpdates",
            (
                200,
                r#"{"ok":true,"result":[{"update_id":10,"message":{"text":"/start","chat":{"id":42,"username":"alice","first_name":"Alice","last_name":"Smith"}}}]}"#,
            ),
        ),
        ("sendMessage", (200, r#"{"ok":true,"result":{}}"#)),
    ]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("poll");
    let telegram = telegram(addr, &path);

    assert_eq!(telegram.poll_updates().await.unwrap(), 1);
    assert_eq!(telegram.active_chats(), 1);

    let registry = Registry::load(&path);
    assert_eq!(registry.offset, 11);
    assert_eq!(registry.chats[0].chat_id, 42);
    assert_eq!(registry.chats[0].username.as_deref(), Some("alice"));
    assert_eq!(registry.chats[0].name, "Alice Smith");

    let requests = last.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/bottest-token/getUpdates?offset=0&timeout=0");
    assert!(requests[1].path.ends_with("/sendMessage"));
    assert!(requests[1].body.contains("You're subscribed"));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn deliver_sends_text_message() {
    let routes = routes([("sendMessage", (200, r#"{"ok":true,"result":{}}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("text");
    write_registry(&path, &[chat(42)]);
    let telegram = telegram(addr, &path);

    telegram.deliver(&update(Vec::new())).await.unwrap();

    let captured = last.lock().unwrap().last().cloned().unwrap();
    assert_eq!(captured.method, "POST");
    assert!(captured.path.ends_with("/sendMessage"));
    assert!(captured.body.contains("\"chat_id\":42"));
    assert!(captured.body.contains("<b>Title</b>\\n\\nSummary."));
    assert!(captured.body.contains("\"parse_mode\":\"HTML\""));
    assert!(captured.body.contains("https://www.isro.gov.in/story.html"));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn deliver_sends_single_photo_with_caption() {
    let routes = routes([("sendPhoto", (200, r#"{"ok":true,"result":{}}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("photo");
    write_registry(&path, &[chat(42)]);
    let telegram = telegram(addr, &path);

    telegram
        .deliver(&update(vec![image("https://example.com/a.webp")]))
        .await
        .unwrap();

    let captured = last.lock().unwrap().last().cloned().unwrap();
    assert!(captured.path.ends_with("/sendPhoto"));
    assert!(captured.body.contains("\"photo\":\"https://example.com/a.webp\""));
    assert!(captured.body.contains("\"caption\":"));
    assert!(captured.body.contains("\"parse_mode\":\"HTML\""));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn deliver_groups_multiple_photos() {
    let routes = routes([("sendMediaGroup", (200, r#"{"ok":true,"result":[]}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("album");
    write_registry(&path, &[chat(42)]);
    let telegram = telegram(addr, &path);

    telegram
        .deliver(&update(vec![
            image("https://example.com/a.webp"),
            image("https://example.com/b.webp"),
        ]))
        .await
        .unwrap();

    let captured = last.lock().unwrap().last().cloned().unwrap();
    assert!(captured.path.ends_with("/sendMediaGroup"));
    assert_eq!(captured.body.matches("\"type\":\"photo\"").count(), 2);
    assert!(captured.body.contains("\"caption\":"));
    assert!(captured.body.contains("\"parse_mode\":\"HTML\""));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn blocked_chat_is_deactivated() {
    let routes = routes([(
        "sendMessage",
        (
            403,
            r#"{"ok":false,"description":"Forbidden: bot was blocked by the user"}"#,
        ),
    )]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last).await;
    let path = temp("blocked");
    write_registry(&path, &[chat(42)]);
    let telegram = telegram(addr, &path);

    assert!(telegram.deliver(&update(Vec::new())).await.is_err());
    assert_eq!(telegram.active_chats(), 0);
    assert!(!Registry::load(&path).chats[0].active);

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn html_is_escaped_in_messages() {
    let routes = routes([("sendMessage", (200, r#"{"ok":true,"result":{}}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("escape");
    write_registry(&path, &[chat(42)]);
    let telegram = telegram(addr, &path);

    let mut update = update(Vec::new());
    update.text = "A & B\n\nC < D".to_string();
    telegram.deliver(&update).await.unwrap();

    let captured = last.lock().unwrap().last().cloned().unwrap();
    assert!(captured.body.contains("<b>A &amp; B</b>"));
    assert!(captured.body.contains("C &lt; D"));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn alert_goes_only_to_the_admin() {
    let routes = routes([("sendMessage", (200, r#"{"ok":true,"result":{}}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("alert");
    write_registry(&path, &[chat_with(42, "alice"), chat_with(99, "other")]);
    let mut config = TelegramConfig::new("test-token", &path);
    config.api = format!("http://{addr}");
    config.admin_username = Some("Alice".to_string());
    let telegram = Telegram::new(reqwest::Client::new(), config);

    telegram.alert("fetch failed: HTTP 503").await.unwrap();

    let captured = last.lock().unwrap().last().cloned().unwrap();
    assert!(captured.path.ends_with("/sendMessage"));
    assert!(captured.body.contains("\"chat_id\":42"));
    assert!(!captured.body.contains("\"chat_id\":99"));
    assert!(captured.body.contains("fetch failed: HTTP 503"));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn alert_with_unregistered_admin_is_an_error() {
    let routes = routes([("sendMessage", (200, r#"{"ok":true,"result":{}}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last).await;
    let path = temp("alert-missing");
    write_registry(&path, &[chat_with(42, "alice")]);
    let mut config = TelegramConfig::new("test-token", &path);
    config.api = format!("http://{addr}");
    config.admin_username = Some("nobody".to_string());
    let telegram = Telegram::new(reqwest::Client::new(), config);

    assert!(telegram.alert("boom").await.is_err());

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn deliver_broadcasts_to_every_active_chat() {
    let routes = routes([("sendMessage", (200, r#"{"ok":true,"result":{}}"#))]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last.clone()).await;
    let path = temp("broadcast");
    write_registry(&path, &[chat(42), chat(99)]);
    let telegram = telegram(addr, &path);

    telegram.deliver(&update(Vec::new())).await.unwrap();

    let requests = last.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().any(|r| r.body.contains("\"chat_id\":42")));
    assert!(requests.iter().any(|r| r.body.contains("\"chat_id\":99")));

    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn no_registered_chats_is_an_error() {
    let routes = routes([]);
    let last: Requests = Arc::new(Mutex::new(Vec::new()));
    let addr = serve(routes, last).await;
    let path = temp("empty");
    let telegram = telegram(addr, &path);

    assert!(telegram.deliver(&update(Vec::new())).await.is_err());
}

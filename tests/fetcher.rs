use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use isro_notifications::fetcher::{FetchEvent, Fetcher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Response {
    status: u16,
    body: Vec<u8>,
}

type State = Arc<Mutex<Response>>;

/// Minimal HTTP/1.1 server whose response can be swapped between polls.
async fn serve(state: State) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let state = state.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let (status, body) = {
                    let state = state.lock().unwrap();
                    (state.status, state.body.clone())
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

fn dev(name: &str) -> Vec<u8> {
    let path = format!("{}/.dev/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn validate(body: &[u8]) -> Result<(), String> {
    if body.len() < 512 {
        return Err("response too small".into());
    }
    Ok(())
}

#[tokio::test]
async fn fetcher_reports_baseline_change_and_failures() {
    let a = dev("isro-2026-09-06-16-42-25.html");
    let b = dev("isro-2026-09-09-19-15-54.html");
    let state: State = Arc::new(Mutex::new(Response {
        status: 200,
        body: a,
    }));
    let addr = serve(state.clone()).await;
    let mut fetcher = Fetcher::new(reqwest::Client::new(), format!("http://{addr}/"), validate);

    assert!(matches!(fetcher.poll().await, FetchEvent::Baseline { .. }));
    assert!(matches!(fetcher.poll().await, FetchEvent::Unchanged { .. }));

    state.lock().unwrap().body = b.clone();
    let FetchEvent::Changed { previous, current } = fetcher.poll().await else {
        panic!("expected a change");
    };
    assert_ne!(previous.checksum, current.checksum);

    // HTTP error: reported, cache untouched.
    state.lock().unwrap().status = 500;
    assert!(matches!(fetcher.poll().await, FetchEvent::Failed { .. }));

    state.lock().unwrap().status = 200;
    assert!(matches!(fetcher.poll().await, FetchEvent::Unchanged { .. }));

    // 200 but blocked page: rejected by the validator, cache untouched.
    state.lock().unwrap().body = b"<html><body>Access Denied</body></html>".to_vec();
    assert!(matches!(fetcher.poll().await, FetchEvent::Failed { .. }));

    state.lock().unwrap().body = b;
    assert!(matches!(fetcher.poll().await, FetchEvent::Unchanged { .. }));
}

#[tokio::test]
async fn fetcher_reports_download_failure() {
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let mut fetcher = Fetcher::new(reqwest::Client::new(), format!("http://{addr}/"), validate);

    assert!(matches!(fetcher.poll().await, FetchEvent::Failed { .. }));
    assert!(fetcher.last().is_none());
}

#[tokio::test]
async fn invalid_first_response_is_not_cached() {
    let state: State = Arc::new(Mutex::new(Response {
        status: 200,
        body: b"<html><body>Access Denied</body></html>".to_vec(),
    }));
    let addr = serve(state.clone()).await;
    let mut fetcher = Fetcher::new(reqwest::Client::new(), format!("http://{addr}/"), validate);

    assert!(matches!(fetcher.poll().await, FetchEvent::Failed { .. }));
    assert!(fetcher.last().is_none());

    state.lock().unwrap().body = dev("isro-2026-09-10-07-54-05.html");
    assert!(matches!(fetcher.poll().await, FetchEvent::Baseline { .. }));
}

use std::sync::{Arc, Mutex};

use isro_notifications::analytics::Analytics;
use isro_notifications::web::{self, WebState};
use tokio::net::TcpListener;

async fn start(state: WebState) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(web::serve(listener, state));
    format!("http://{addr}")
}

#[tokio::test]
async fn analytics_api_returns_state() {
    let analytics = Arc::new(Mutex::new(Analytics::new()));
    analytics.lock().unwrap().record_run("ok");

    let base = start(WebState {
        analytics,
        telegram: None,
        admin_username: Some("alice".to_string()),
    })
    .await;

    let body = reqwest::get(format!("{base}/api/analytics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["updates_sent"], 0);
    assert_eq!(json["admin_username"], "alice");
    assert!(json["users"].as_array().unwrap().is_empty());
    assert!(json["last_run"].is_string());
}

#[tokio::test]
async fn index_is_served() {
    let base = start(WebState {
        analytics: Arc::new(Mutex::new(Analytics::new())),
        telegram: None,
        admin_username: None,
    })
    .await;

    let response = reqwest::get(format!("{base}/")).await.unwrap();
    assert!(response.status().is_success());
    assert!(response.text().await.unwrap().contains("ISRO Notifications"));
}

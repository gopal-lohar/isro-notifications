use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::analytics::{Analytics, RecentUpdate};
use crate::telegram::Telegram;

pub struct WebState {
    pub analytics: Arc<Mutex<Analytics>>,
    pub telegram: Option<Telegram>,
    pub admin_username: Option<String>,
}

#[derive(Serialize)]
struct UserView {
    chat_id: i64,
    username: Option<String>,
    name: String,
    active: bool,
    admin: bool,
}

#[derive(Serialize)]
struct ApiResponse {
    started_at: DateTime<Utc>,
    last_run: Option<DateTime<Utc>>,
    status: String,
    updates_sent: u64,
    recent: Vec<RecentUpdate>,
    users: Vec<UserView>,
    admin_username: Option<String>,
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/style.css", get(style))
        .route("/app.js", get(script))
        .route("/api/analytics", get(analytics))
        .with_state(Arc::new(state))
}

pub async fn serve(listener: tokio::net::TcpListener, state: WebState) -> Result<(), String> {
    axum::serve(listener, router(state))
        .await
        .map_err(|e| format!("serve: {e}"))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}

async fn style() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css")],
        include_str!("../web/style.css"),
    )
        .into_response()
}

async fn script() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("../web/app.js"),
    )
        .into_response()
}

async fn analytics(State(state): State<Arc<WebState>>) -> Json<ApiResponse> {
    let snapshot = state.analytics.lock().unwrap().snapshot();
    let users = state
        .telegram
        .as_ref()
        .map(Telegram::chats)
        .unwrap_or_default()
        .into_iter()
        .map(|chat| UserView {
            admin: is_admin(&state.admin_username, chat.username.as_deref()),
            chat_id: chat.chat_id,
            username: chat.username,
            name: chat.name,
            active: chat.active,
        })
        .collect();

    Json(ApiResponse {
        started_at: snapshot.started_at,
        last_run: snapshot.last_run,
        status: snapshot.status,
        updates_sent: snapshot.updates_sent,
        recent: snapshot.recent,
        users,
        admin_username: state.admin_username.clone(),
    })
}

fn is_admin(admin: &Option<String>, username: Option<&str>) -> bool {
    match (admin.as_deref(), username) {
        (Some(admin), Some(username)) => admin.eq_ignore_ascii_case(username),
        _ => false,
    }
}

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Client;
use serde_json::{Map, Value, json};
use tracing::{info, warn};

use crate::enrich::{Attachment, AttachmentKind, Update};
use crate::pipeline::Deliverer;
use crate::registry::{Chat, Registry};

pub const DEFAULT_API: &str = "https://api.telegram.org";
const CAPTION_LIMIT: usize = 1024;
const ALBUM_LIMIT: usize = 10;

pub struct TelegramConfig {
    pub token: String,
    pub registry_path: PathBuf,
    pub api: String,
    /// Username (without @) that also receives failure alerts.
    pub admin_username: Option<String>,
}

impl TelegramConfig {
    pub fn new(token: impl Into<String>, registry_path: impl Into<PathBuf>) -> Self {
        Self {
            token: token.into(),
            registry_path: registry_path.into(),
            api: DEFAULT_API.to_string(),
            admin_username: None,
        }
    }
}

#[derive(Clone)]
pub struct Telegram {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    token: String,
    api: String,
    registry_path: PathBuf,
    registry: Mutex<Registry>,
    admin_username: Option<String>,
}

impl Telegram {
    pub fn new(client: Client, config: TelegramConfig) -> Self {
        let registry = Registry::load(&config.registry_path);
        Self {
            inner: Arc::new(Inner {
                client,
                token: config.token,
                api: config.api,
                registry_path: config.registry_path,
                registry: Mutex::new(registry),
                admin_username: config.admin_username,
            }),
        }
    }

    pub fn active_chats(&self) -> usize {
        self.inner.registry.lock().unwrap().active().count()
    }

    /// Drain pending `getUpdates` and merge new chats into the registry.
    pub async fn poll_updates(&self) -> Result<usize, String> {
        let offset = self.inner.registry.lock().unwrap().offset;
        let url = format!("{}/bot{}/getUpdates", self.inner.api, self.inner.token);
        let response = self
            .inner
            .client
            .get(&url)
            .query(&[("offset", offset.to_string()), ("timeout", "0".to_string())])
            .send()
            .await
            .map_err(|e| format!("getUpdates failed: {e}"))?;
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("getUpdates response: {e}"))?;
        if body["ok"] != true {
            return Err(format!(
                "getUpdates: {}",
                body["description"].as_str().unwrap_or("unknown error")
            ));
        }

        let mut added = 0;
        let mut changed = false;
        let mut registry = self.inner.registry.lock().unwrap();
        for update in body["result"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = update["update_id"].as_i64()
                && id + 1 > registry.offset
            {
                registry.offset = id + 1;
                changed = true;
            }
            let chat = &update["message"]["chat"];
            let Some(chat_id) = chat["id"].as_i64() else {
                continue;
            };
            let name = [chat["first_name"].as_str(), chat["last_name"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let entry = Chat {
                chat_id,
                username: chat["username"].as_str().map(str::to_string),
                name: if name.is_empty() {
                    chat_id.to_string()
                } else {
                    name
                },
                active: true,
            };
            if registry.upsert(entry) {
                added += 1;
                changed = true;
            }
        }
        if changed {
            registry
                .save(&self.inner.registry_path)
                .map_err(|e| format!("registry save: {e}"))?;
        }
        Ok(added)
    }

    async fn send(&self, chat: &Chat, update: &Update) -> Result<(), SendError> {
        let text = compose_text(update);
        let caption = (text.chars().count() <= CAPTION_LIMIT).then_some(text.as_str());
        if caption.is_none() {
            self.send_message(chat.chat_id, &text).await?;
        }

        if update.attachments.is_empty() {
            if caption.is_some() {
                self.send_message(chat.chat_id, &text).await?;
            }
            return Ok(());
        }
        self.send_media(chat.chat_id, &update.attachments, caption)
            .await
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), SendError> {
        self.call(
            "sendMessage",
            json!({ "chat_id": chat_id, "text": text, "disable_web_page_preview": true }),
        )
        .await
        .map(|_| ())
    }

    async fn send_media(
        &self,
        chat_id: i64,
        attachments: &[Attachment],
        caption: Option<&str>,
    ) -> Result<(), SendError> {
        if attachments.len() == 1 {
            return self.send_single(chat_id, &attachments[0], caption).await;
        }

        for (index, chunk) in attachments.chunks(ALBUM_LIMIT).enumerate() {
            if chunk.len() == 1 {
                let caption = if index == 0 { caption } else { None };
                self.send_single(chat_id, &chunk[0], caption).await?;
                continue;
            }
            let media: Vec<Value> = chunk
                .iter()
                .enumerate()
                .map(|(i, attachment)| {
                    let mut item = json!({
                        "type": media_type(attachment.kind),
                        "media": attachment.url,
                    });
                    if index == 0 && i == 0 && let Some(caption) = caption {
                        item["caption"] = json!(caption);
                    }
                    item
                })
                .collect();
            self.call("sendMediaGroup", json!({ "chat_id": chat_id, "media": media }))
                .await?;
        }
        Ok(())
    }

    async fn send_single(
        &self,
        chat_id: i64,
        attachment: &Attachment,
        caption: Option<&str>,
    ) -> Result<(), SendError> {
        let (method, field) = match attachment.kind {
            AttachmentKind::Image => ("sendPhoto", "photo"),
            AttachmentKind::Video => ("sendVideo", "video"),
        };
        let mut body = Map::new();
        body.insert("chat_id".into(), json!(chat_id));
        body.insert(field.into(), json!(attachment.url));
        if let Some(caption) = caption {
            body.insert("caption".into(), json!(caption));
        }

        match self.call(method, Value::Object(body)).await {
            Ok(_) => Ok(()),
            // Telegram rejects some formats (e.g. webp) as photos; fall back to a document.
            Err(error) if !error.blocked => {
                let mut body = Map::new();
                body.insert("chat_id".into(), json!(chat_id));
                body.insert("document".into(), json!(attachment.url));
                if let Some(caption) = caption {
                    body.insert("caption".into(), json!(caption));
                }
                self.call("sendDocument", Value::Object(body))
                    .await
                    .map(|_| ())
            }
            Err(error) => Err(error),
        }
    }

    async fn call(&self, method: &str, body: Value) -> Result<Value, SendError> {
        let url = format!("{}/bot{}/{}", self.inner.api, self.inner.token, method);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .inner
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| SendError::new(format!("{method}: {e}")))?;
            let status = response.status();
            let body: Value = response
                .json()
                .await
                .map_err(|e| SendError::new(format!("{method} response: {e}")))?;
            if body["ok"] == true {
                return Ok(body);
            }

            let description = body["description"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            if status == 429 && attempt < 2 {
                let retry = body["parameters"]["retry_after"].as_u64().unwrap_or(1);
                warn!(method, retry, "telegram rate limited, retrying");
                tokio::time::sleep(Duration::from_secs(retry)).await;
                continue;
            }
            let blocked = status == 403 || description.to_lowercase().contains("blocked");
            return Err(SendError {
                message: format!("{method}: {description}"),
                blocked,
            });
        }
    }
}

impl Deliverer for Telegram {
    fn deliver<'a>(
        &'a self,
        update: &'a Update,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let chats: Vec<Chat> = self
                .inner
                .registry
                .lock()
                .unwrap()
                .active()
                .cloned()
                .collect();
            if chats.is_empty() {
                return Err("no active chats registered".to_string());
            }

            let mut errors = Vec::new();
            for chat in chats {
                if let Err(error) = self.send(&chat, update).await {
                    if error.blocked {
                        let mut registry = self.inner.registry.lock().unwrap();
                        registry.deactivate(chat.chat_id);
                        let _ = registry.save(&self.inner.registry_path);
                        info!(chat_id = chat.chat_id, "chat blocked the bot, deactivated");
                    }
                    errors.push(format!("{}: {}", chat.chat_id, error.message));
                }
            }

            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            }
        })
    }

    fn alert<'a>(
        &'a self,
        message: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let Some(username) = self.inner.admin_username.as_deref() else {
                return Ok(());
            };
            let chat_id = self
                .inner
                .registry
                .lock()
                .unwrap()
                .chats
                .iter()
                .find(|chat| {
                    chat.username
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(username))
                })
                .map(|chat| chat.chat_id);
            let Some(chat_id) = chat_id else {
                return Err(format!("admin @{username} is not registered"));
            };
            self.send_message(chat_id, message)
                .await
                .map_err(|error| error.message)
        })
    }
}

struct SendError {
    message: String,
    blocked: bool,
}

impl SendError {
    fn new(message: String) -> Self {
        Self {
            message,
            blocked: false,
        }
    }
}

fn compose_text(update: &Update) -> String {
    match &update.link {
        Some(link) => format!("{}\n\n{}", update.text, link),
        None => update.text.clone(),
    }
}

fn media_type(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "photo",
        AttachmentKind::Video => "video",
    }
}

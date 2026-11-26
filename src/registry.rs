use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    pub chat_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_active() -> bool {
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    /// `getUpdates` cursor, so a restart never reprocesses old messages.
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub chats: Vec<Chat>,
}

impl Registry {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }

    /// Insert or refresh a chat. Returns true when it was new.
    pub fn upsert(&mut self, chat: Chat) -> bool {
        match self.chats.iter_mut().find(|c| c.chat_id == chat.chat_id) {
            Some(existing) => {
                existing.username = chat.username;
                existing.name = chat.name;
                existing.active = true;
                false
            }
            None => {
                self.chats.push(chat);
                true
            }
        }
    }

    pub fn active(&self) -> impl Iterator<Item = &Chat> {
        self.chats.iter().filter(|chat| chat.active)
    }

    pub fn deactivate(&mut self, chat_id: i64) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.chat_id == chat_id) {
            chat.active = false;
        }
    }
}

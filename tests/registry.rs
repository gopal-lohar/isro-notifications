use std::path::PathBuf;

use isro_notifications::registry::{Chat, Registry};

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "isro-registry-{name}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn chat(id: i64, username: &str, name: &str) -> Chat {
    Chat {
        chat_id: id,
        username: Some(username.to_string()),
        name: name.to_string(),
        active: true,
    }
}

#[test]
fn upsert_adds_then_refreshes() {
    let mut registry = Registry::default();
    assert!(registry.upsert(chat(1, "a", "A")));
    assert!(!registry.upsert(chat(1, "b", "B")));
    assert_eq!(registry.chats.len(), 1);
    assert_eq!(registry.chats[0].username.as_deref(), Some("b"));
    assert_eq!(registry.chats[0].name, "B");
}

#[test]
fn deactivate_hides_chat_from_active() {
    let mut registry = Registry::default();
    registry.upsert(chat(1, "a", "A"));
    registry.upsert(chat(2, "b", "B"));
    registry.deactivate(1);

    let active: Vec<i64> = registry.active().map(|chat| chat.chat_id).collect();
    assert_eq!(active, vec![2]);
}

#[test]
fn save_and_load_roundtrip() {
    let path = temp("roundtrip");
    let mut registry = Registry {
        offset: 42,
        chats: Vec::new(),
    };
    registry.upsert(chat(7, "alice", "Alice Smith"));
    registry.save(&path).unwrap();

    let loaded = Registry::load(&path);
    assert_eq!(loaded.offset, 42);
    assert_eq!(loaded.chats, registry.chats);

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"username\": \"alice\""));
    assert!(text.contains("\"name\": \"Alice Smith\""));

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn missing_file_loads_empty() {
    let path = temp("missing");
    let registry = Registry::load(&path);
    assert!(registry.chats.is_empty());
    assert_eq!(registry.offset, 0);
}

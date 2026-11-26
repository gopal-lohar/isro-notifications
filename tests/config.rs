use std::path::PathBuf;

use isro_notifications::config::Config;

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "isro-config-{name}-{}.yaml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn defaults_are_used_when_file_is_missing() {
    let config = Config::load(&temp("missing")).unwrap();

    assert_eq!(config.isro.url, "https://www.isro.gov.in");
    assert_eq!(config.isro.base_url, "https://www.isro.gov.in");
    assert_eq!(config.isro.poll_interval_secs, 1800);
    assert_eq!(config.summary.max_chars, 1024);
    assert_eq!(
        config.telegram.registry,
        PathBuf::from("./.state/telegram.json")
    );
    assert!(config.telegram.admin_username.is_none());
    assert!(config.telegram.welcome.contains("subscribed"));
    assert_eq!(config.telegram.alerts.fetch_failed, "fetch failed: {error}");
}

#[test]
fn yaml_overrides_defaults() {
    let path = temp("custom");
    std::fs::write(
        &path,
        r#"
isro:
  url: http://127.0.0.1:9000/
  poll_interval_secs: 30
summary:
  max_chars: 500
telegram:
  admin_username: alice
  welcome: "hello"
  alerts:
    fetch_failed: "down: {error}"
"#,
    )
    .unwrap();

    let config = Config::load(&path).unwrap();
    assert_eq!(config.isro.url, "http://127.0.0.1:9000/");
    assert_eq!(config.isro.base_url, "http://127.0.0.1:9000/");
    assert_eq!(config.isro.poll_interval_secs, 30);
    assert_eq!(config.summary.max_chars, 500);
    assert_eq!(config.telegram.admin_username.as_deref(), Some("alice"));
    assert_eq!(config.telegram.welcome, "hello");
    assert_eq!(config.telegram.alerts.fetch_failed, "down: {error}");
    assert_eq!(
        config.telegram.alerts.site_failure,
        "site failure: {message}"
    );

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn base_url_defaults_to_url() {
    let path = temp("base");
    std::fs::write(&path, "isro:\n  url: https://example.com\n").unwrap();

    let config = Config::load(&path).unwrap();
    assert_eq!(config.isro.base_url, "https://example.com");

    std::fs::remove_file(&path).unwrap();
}

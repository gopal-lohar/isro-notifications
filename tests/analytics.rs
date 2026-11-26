use isro_notifications::analytics::Analytics;
use isro_notifications::diff::Kind;
use isro_notifications::enrich::Update;

#[test]
fn counts_updates_and_keeps_the_latest_first() {
    let mut analytics = Analytics::new();
    analytics.record_run("ok");

    for title in ["First", "Second"] {
        analytics.record_update(&Update {
            kind: Kind::Flash,
            link: Some("https://www.isro.gov.in/x.html".to_string()),
            text: format!("{title}\n\nSummary."),
            attachments: Vec::new(),
        });
    }

    let snapshot = analytics.snapshot();
    assert_eq!(snapshot.updates_sent, 2);
    assert_eq!(snapshot.status, "ok");
    assert!(snapshot.last_run.is_some());
    assert_eq!(snapshot.recent[0].title, "Second");
    assert_eq!(snapshot.recent[1].title, "First");
    assert_eq!(snapshot.recent[0].kind, "Flash");
}

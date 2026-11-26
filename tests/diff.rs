use isro_notifications::diff::{self, DiffEvent, Kind, NewsItem};

const BASE: &str = "https://www.isro.gov.in";

fn dev(name: &str) -> Vec<u8> {
    let path = format!("{}/.dev/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn news(old: &str, new: &str) -> Vec<NewsItem> {
    match diff::compare(&dev(old), &dev(new), BASE) {
        DiffEvent::News { items } => items,
        other => panic!("expected news, got {other:?}"),
    }
}

#[test]
fn identical_snapshots_report_no_news() {
    let old = dev("isro-2026-09-09-19-15-54.html");
    let new = dev("isro-2026-09-10-07-54-05.html");
    assert_eq!(diff::compare(&old, &new, BASE), DiffEvent::NoNews);
}

#[test]
fn flash_additions_are_reported() {
    let items = news(
        "isro-2026-08-29-17-06-58.html",
        "isro-2026-09-04-22-01-32.html",
    );

    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item.kind == Kind::Flash));
    assert_eq!(
        items[0].title,
        "GSLV-F17 has successfully accomplished its mission, placing EOS-05 into the intended orbit."
    );
    assert_eq!(
        items[0].link.as_deref(),
        Some("https://www.isro.gov.in/Mission_GSLVF17.html")
    );
    assert_eq!(
        items[1].title,
        "GSLV-F17/EOS-05 Mission is scheduled for launch on 04 Sept 2026 at 02:55 AM IST"
    );
}

#[test]
fn latest_additions_are_reported() {
    let items = news(
        "isro-2026-09-06-16-42-25.html",
        "isro-2026-09-09-19-15-54.html",
    );

    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|item| item.kind == Kind::Latest));
    let titles: Vec<&str> = items.iter().map(|item| item.title.as_str()).collect();
    for expected in [
        "ZeeDP 2.0 Space Science Meet held at SAC, Ahmedabad",
        "National Anubhav Awards, 2026",
        "Third and final orbit manoeuvring of EOS-05 spacecraft",
        "Second orbit manoeuvring of EOS-05 spacecraft",
    ] {
        assert!(titles.contains(&expected), "missing {expected:?} in {titles:?}");
    }
}

#[test]
fn removals_are_not_reported() {
    let items = news(
        "isro-2026-09-10-07-54-05.html",
        "isro-2026-11-26-18-59-30.html",
    );

    let flash: Vec<&NewsItem> = items.iter().filter(|i| i.kind == Kind::Flash).collect();
    let latest = items.iter().filter(|i| i.kind == Kind::Latest).count();
    assert_eq!(flash.len(), 1);
    assert_eq!(
        flash[0].title,
        "GSLV-F18 has successfully placed NVS-03 in the intended orbit."
    );
    assert_eq!(latest, 25);
    assert!(
        items
            .iter()
            .all(|item| !item.title.contains("GSLV-F17 has successfully")),
        "removed flash news leaked into the report"
    );
}

#[test]
fn every_reported_item_carries_its_link() {
    let items = news(
        "isro-2026-09-10-07-54-05.html",
        "isro-2026-11-26-18-59-30.html",
    );

    for item in &items {
        let link = item.link.as_deref().unwrap_or_else(|| panic!("no link for {:?}", item.title));
        assert!(
            link.starts_with("https://www.isro.gov.in/"),
            "unresolved link {link:?} for {:?}",
            item.title
        );
    }
}

#[test]
fn sup_tags_are_stripped_from_titles() {
    let items = news(
        "isro-2026-09-10-07-54-05.html",
        "isro-2026-11-26-18-59-30.html",
    );

    let icc = items
        .iter()
        .find(|item| item.title.contains("Thirtieth Session"))
        .expect("ICC-30 item");
    assert!(icc.title.contains("26th & 27th October, 2026"), "{}", icc.title);
    assert!(!icc.title.contains('<'));
}

#[test]
fn blocked_page_is_a_failure() {
    let old = dev("isro-2026-09-10-07-54-05.html");
    let blocked = b"<html><head><title>Just a moment...</title></head><body>Checking your browser before accessing. Cloudflare</body></html>";
    assert!(matches!(
        diff::compare(&old, blocked, BASE),
        DiffEvent::Failure { .. }
    ));
}

#[test]
fn maintenance_page_is_a_failure() {
    let old = dev("isro-2026-09-10-07-54-05.html");
    let page = b"<html><body><h1>Service Unavailable</h1><p>The site is under maintenance.</p></body></html>";
    assert!(matches!(
        diff::compare(&old, page, BASE),
        DiffEvent::Failure { .. }
    ));
}

#[test]
fn validate_accepts_real_page_and_rejects_error_pages() {
    assert!(diff::validate(&dev("isro-2026-09-10-07-54-05.html")).is_ok());
    assert!(diff::validate(b"<html><body>Access Denied</body></html>").is_err());
    assert!(diff::validate(b"tiny").is_err());
}

#[test]
fn formatted_news_is_plain_text() {
    let items = news(
        "isro-2026-09-06-16-42-25.html",
        "isro-2026-09-09-19-15-54.html",
    );
    let text = DiffEvent::News { items }.to_string();

    assert!(!text.contains('<'), "{text}");
    assert!(text.contains("[Latest News] ZeeDP 2.0 Space Science Meet held at SAC, Ahmedabad"));
    assert!(text.contains("https://www.isro.gov.in/ZeeDP2_Space_Science_Meet.html"));
}

use std::collections::HashSet;
use std::fmt;

use scraper::{Html, Selector};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Flash,
    Latest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsItem {
    pub kind: Kind,
    pub title: String,
    pub link: Option<String>,
}

impl NewsItem {
    /// Identity of an item within its section: its link when it has one, otherwise its title.
    fn key(&self) -> (Kind, &str) {
        (self.kind, self.link.as_deref().unwrap_or(&self.title))
    }
}

impl fmt::Display for NewsItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            Kind::Flash => "Flash News",
            Kind::Latest => "Latest News",
        };
        match &self.link {
            Some(link) => write!(f, "[{kind}] {}\n  {link}", self.title),
            None => write!(f, "[{kind}] {}", self.title),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffEvent {
    /// The new response is not a usable ISRO page (blocked, maintenance, error page).
    Failure { message: String },
    /// Genuine page change with one or more new flash/latest news items.
    News { items: Vec<NewsItem> },
    /// Genuine page change, but nothing was added (removals and edits only).
    NoNews,
}

impl fmt::Display for DiffEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiffEvent::Failure { message } => write!(f, "site failure: {message}"),
            DiffEvent::NoNews => write!(f, "page changed but no new news"),
            DiffEvent::News { items } => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{item}")?;
                }
                Ok(())
            }
        }
    }
}

/// Reject responses that are not a usable ISRO homepage.
pub fn validate(body: &[u8]) -> Result<(), String> {
    parse_page(body).map(|_| ())
}

/// Compare two snapshots and report only additions.
pub fn compare(old: &[u8], new: &[u8], base_url: &str) -> DiffEvent {
    let old = match parse_page(old) {
        Ok(page) => page,
        Err(message) => return DiffEvent::Failure { message },
    };
    let new = match parse_page(new) {
        Ok(page) => page,
        Err(message) => return DiffEvent::Failure { message },
    };

    let known: HashSet<(Kind, &str)> = old.items.iter().map(NewsItem::key).collect();
    let items: Vec<NewsItem> = new
        .items
        .into_iter()
        .filter(|item| !known.contains(&item.key()))
        .map(|mut item| {
            item.link = item.link.map(|href| absolute(base_url, &href));
            item
        })
        .collect();

    if items.is_empty() {
        DiffEvent::NoNews
    } else {
        DiffEvent::News { items }
    }
}

struct Page {
    items: Vec<NewsItem>,
}

fn parse_page(body: &[u8]) -> Result<Page, String> {
    if body.len() < 512 {
        return Err(format!("response too small ({} bytes)", body.len()));
    }
    let html = String::from_utf8_lossy(body);
    let doc = Html::parse_document(&html);

    let mut items = extract_flash(&doc);
    items.extend(extract_latest(&doc));
    if items.is_empty() {
        return Err(classify_failure(&html));
    }
    Ok(Page { items })
}

const FAILURE_MARKERS: &[(&str, &str)] = &[
    ("access denied", "access denied"),
    ("403 forbidden", "403 forbidden"),
    ("service unavailable", "service unavailable"),
    ("bad gateway", "bad gateway"),
    ("just a moment", "cloudflare challenge"),
    ("attention required", "cloudflare block"),
    ("cloudflare", "cloudflare block"),
    ("captcha", "captcha challenge"),
    ("request blocked", "request blocked"),
    ("too many requests", "rate limited"),
    ("under maintenance", "maintenance page"),
    ("temporarily unavailable", "temporarily unavailable"),
    ("site is not available", "site not available"),
];

fn classify_failure(html: &str) -> String {
    let lower = html.to_lowercase();
    for (marker, label) in FAILURE_MARKERS {
        if lower.contains(marker) {
            return format!("site returned an error page ({label})");
        }
    }
    "unrecognised page: no flash news or latest news found".to_string()
}

fn extract_flash(doc: &Html) -> Vec<NewsItem> {
    let flash = Selector::parse("p.flashContent span.out").expect("flash selector");
    let anchor = Selector::parse("a[href]").expect("anchor selector");

    doc.select(&flash)
        .filter_map(|el| {
            let title = el
                .value()
                .attr("title")
                .map(normalize)
                .filter(|t| !t.is_empty())
                .or_else(|| {
                    let text = normalize(&el.text().collect::<String>());
                    (!text.is_empty()).then_some(text)
                })?;
            let link = el
                .select(&anchor)
                .next()
                .and_then(|a| a.value().attr("href"))
                .or_else(|| el.value().attr("onclick").and_then(onclick_url))
                .map(str::to_string);
            Some(NewsItem {
                kind: Kind::Flash,
                title,
                link,
            })
        })
        .collect()
}

fn extract_latest(doc: &Html) -> Vec<NewsItem> {
    let item = Selector::parse("#accordionExample .accordion-item").expect("item selector");
    let title = Selector::parse("span.open-blink").expect("title selector");
    let anchor = Selector::parse("a[href]").expect("anchor selector");
    let onclick = Selector::parse("span.out[onclick]").expect("onclick selector");

    doc.select(&item)
        .filter_map(|el| {
            let title = normalize(&el.select(&title).next()?.text().collect::<String>());
            if title.is_empty() {
                return None;
            }
            let link = el
                .select(&anchor)
                .next()
                .and_then(|a| a.value().attr("href"))
                .or_else(|| {
                    el.select(&onclick)
                        .next()
                        .and_then(|s| s.value().attr("onclick"))
                        .and_then(onclick_url)
                })
                .map(str::to_string);
            Some(NewsItem {
                kind: Kind::Latest,
                title,
                link,
            })
        })
        .collect()
}

/// `outsideAlert('https://...')` -> `https://...`
fn onclick_url(onclick: &str) -> Option<&str> {
    onclick.split('\'').nth(1).filter(|url| url.starts_with("http"))
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn absolute(base: &str, href: &str) -> String {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|base| base.join(href).ok())
        .map(|url| url.to_string())
        .unwrap_or_else(|| href.to_string())
}

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use futures::stream::{self, StreamExt};
use reqwest::{Client, Url};
use scraper::{Html, Selector};
use tracing::warn;

use crate::diff::{Kind, NewsItem};

#[derive(Clone, Debug)]
pub struct Config {
    /// Maximum length of the final text, in characters.
    pub max_chars: usize,
    /// Site the updates belong to; links on this host are fetched and summarised.
    pub base_url: String,
    /// How many articles to fetch and summarise at once.
    pub concurrency: usize,
}

impl Config {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            max_chars: 1024,
            base_url: base_url.into(),
            concurrency: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Video,
}

impl fmt::Display for AttachmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttachmentKind::Image => write!(f, "image"),
            AttachmentKind::Video => write!(f, "video"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub url: String,
}

/// What the next layer receives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub kind: Kind,
    pub link: Option<String>,
    /// Message body: the title for plain links, or `title\n\nsummary` for summarised articles.
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl fmt::Display for Update {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)?;
        if let Some(link) = &self.link {
            write!(f, "\n{link}")?;
        }
        for attachment in &self.attachments {
            write!(f, "\n[{}] {}", attachment.kind, attachment.url)?;
        }
        Ok(())
    }
}

/// Pluggable text summariser. The layer never talks to a provider directly.
pub trait Summarizer: Send + Sync {
    fn summarize<'a>(
        &'a self,
        title: &'a str,
        text: &'a str,
        max_chars: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Turn diff items into updates: external links pass through, internal links are
/// fetched and summarised when a summariser is available.
pub async fn enrich(
    client: &Client,
    items: &[NewsItem],
    cfg: &Config,
    summarizer: Option<&dyn Summarizer>,
) -> Vec<Update> {
    stream::iter(items)
        .map(|item| enrich_one(client, item, cfg, summarizer))
        .buffered(cfg.concurrency.max(1))
        .collect()
        .await
}

async fn enrich_one(
    client: &Client,
    item: &NewsItem,
    cfg: &Config,
    summarizer: Option<&dyn Summarizer>,
) -> Update {
    let Some(link) = item.link.as_deref() else {
        return passthrough(item, cfg.max_chars);
    };
    // The link is appended to the message, so it shares the character budget.
    let budget = cfg.max_chars.saturating_sub(link.chars().count() + 2);
    let Some(summarizer) = summarizer else {
        return passthrough(item, budget);
    };
    if !is_internal(&cfg.base_url, link) {
        return passthrough(item, budget);
    }

    let article = match fetch_article(client, link).await {
        Ok(article) => article,
        Err(error) => {
            warn!(%link, %error, "article fetch failed, sending link only");
            return passthrough(item, budget);
        }
    };

    let text = match summarize_with_retry(summarizer, &item.title, &article.text, budget).await {
        Ok(text) => text,
        Err(error) => {
            warn!(%link, %error, "summary failed, sending link only");
            return passthrough(item, budget);
        }
    };

    Update {
        kind: item.kind,
        link: item.link.clone(),
        text,
        attachments: article.attachments,
    }
}

fn passthrough(item: &NewsItem, budget: usize) -> Update {
    Update {
        kind: item.kind,
        link: item.link.clone(),
        text: trim_to(&item.title, budget),
        attachments: Vec::new(),
    }
}

async fn summarize_with_retry(
    summarizer: &dyn Summarizer,
    title: &str,
    text: &str,
    max_chars: usize,
) -> Result<String, String> {
    let first = format_summary(&summarizer.summarize(title, text, max_chars).await?, title);
    if first.chars().count() <= max_chars {
        return Ok(first);
    }

    // One retry with a tighter budget, then hard-trim.
    let second = match summarizer.summarize(title, text, max_chars / 2).await {
        Ok(text) => format_summary(&text, title),
        Err(_) => first,
    };
    if second.chars().count() <= max_chars {
        return Ok(second);
    }
    Ok(trim_to(&second, max_chars))
}

fn is_internal(base: &str, link: &str) -> bool {
    match (Url::parse(base), Url::parse(link)) {
        (Ok(base), Ok(link)) => base.host_str() == link.host_str(),
        _ => false,
    }
}

struct Article {
    text: String,
    attachments: Vec<Attachment>,
}

async fn fetch_article(client: &Client, url: &str) -> Result<Article, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("body read failed: {e}"))?;
    Ok(parse_article(&body, url))
}

fn parse_article(html: &str, url: &str) -> Article {
    let doc = Html::parse_document(html);

    let paragraphs = Selector::parse("p.pageContent, p.pMain").expect("paragraph selector");
    let text = doc
        .select(&paragraphs)
        .map(|el| normalize(&el.text().collect::<String>()))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    Article {
        text,
        attachments: extract_attachments(&doc, url),
    }
}

fn extract_attachments(doc: &Html, base: &str) -> Vec<Attachment> {
    let mut attachments = Vec::new();
    let mut seen = HashSet::new();

    let images = Selector::parse("img[src]").expect("image selector");
    for el in doc.select(&images) {
        let src = el.value().attr("src").unwrap_or_default();
        let alt = el.value().attr("alt").unwrap_or_default();
        if src.starts_with("data:") || src.contains("/icon/") || alt.eq_ignore_ascii_case("pdf icon")
        {
            continue;
        }
        push_attachment(&mut attachments, &mut seen, AttachmentKind::Image, base, src);
    }

    let media = Selector::parse("video[src], video source[src], iframe[src]").expect("media selector");
    for el in doc.select(&media) {
        let src = el.value().attr("src").unwrap_or_default();
        if el.value().name() == "iframe" && !is_video_embed(src) {
            continue;
        }
        push_attachment(&mut attachments, &mut seen, AttachmentKind::Video, base, src);
    }

    attachments
}

fn is_video_embed(src: &str) -> bool {
    ["youtube.com", "youtu.be", "vimeo.com", "player."]
        .iter()
        .any(|host| src.contains(host))
}

fn push_attachment(
    attachments: &mut Vec<Attachment>,
    seen: &mut HashSet<String>,
    kind: AttachmentKind,
    base: &str,
    src: &str,
) {
    if src.is_empty() {
        return;
    }
    let url = Url::parse(base)
        .ok()
        .and_then(|base| base.join(src).ok())
        .map(|url| url.to_string())
        .unwrap_or_else(|| src.to_string());
    if seen.insert(url.clone()) {
        attachments.push(Attachment { kind, url });
    }
}

/// Force the `title\n\nsummary` shape and make the update's title authoritative.
fn format_summary(text: &str, title: &str) -> String {
    let text = strip_meta(&normalize(text));
    let body = match text.split_once('\n') {
        Some((_, rest)) => rest.trim_start_matches('\n').to_string(),
        None => text,
    };
    if body.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{body}")
    }
}

/// Drop trailing meta lines the model sometimes adds, e.g. "Character count: 694".
fn strip_meta(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|line| is_meta_line(line)) {
        lines.pop();
    }
    lines.join("\n").trim_end().to_string()
}

fn is_meta_line(line: &str) -> bool {
    let line = line.trim();
    if line.to_lowercase().starts_with("character count") {
        return true;
    }
    let inner = line
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim()
        .to_lowercase();
    let Some(count) = inner
        .strip_suffix(" characters")
        .or_else(|| inner.strip_suffix(" character"))
    else {
        return false;
    };
    !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
}

/// Trim, normalise line endings and collapse runs of blank lines.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newlines = 0;
    for ch in text.trim().chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines > 2 {
                continue;
            }
        } else {
            newlines = 0;
        }
        out.push(ch);
    }
    out
}

fn trim_to(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    if let Some(cut) = out.rfind(char::is_whitespace) {
        out.truncate(cut);
    }
    out.trim_end().to_string()
}

use std::future::Future;
use std::pin::Pin;

use reqwest::Client;
use serde_json::json;

use crate::enrich::Summarizer;

pub const DEFAULT_ENDPOINT: &str = "https://api.sarvam.ai/v1/chat/completions";
pub const DEFAULT_MODEL: &str = "sarvam-105b";

/// Sarvam chat-completions summariser.
pub struct Sarvam {
    client: Client,
    api_key: String,
    model: String,
    endpoint: String,
}

impl Sarvam {
    pub fn new(client: Client, api_key: impl Into<String>) -> Self {
        Self {
            client,
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_string(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

impl Summarizer for Sarvam {
    fn summarize<'a>(
        &'a self,
        title: &'a str,
        text: &'a str,
        max_chars: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let body = json!({
                "model": self.model,
                "messages": [{ "role": "user", "content": prompt(title, text, max_chars) }],
                "temperature": 0.2,
                "reasoning_effort": null,
                "max_tokens": (max_chars / 2).clamp(128, 4096),
            });

            let response = self
                .client
                .post(&self.endpoint)
                .header("api-subscription-key", &self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("sarvam request failed: {e}"))?;

            let status = response.status();
            let body: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("sarvam response: {e}"))?;
            if !status.is_success() {
                return Err(format!("sarvam HTTP {status}: {body}"));
            }

            body["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("sarvam response missing content: {body}"))
        })
    }
}

fn prompt(title: &str, text: &str, max_chars: usize) -> String {
    format!(
        "Format this ISRO update for a notification.\n\
         Rules:\n\
         - First line: the title exactly as given below, character for character. Do not rewrite it, do not change its tense or wording.\n\
         - One blank line.\n\
         - Then a summary of at most 3 sentences that condenses the article's own sentences. Keep the article's wording, tense and terminology; do not rewrite it like a news report, and do not copy it verbatim.\n\
         - The whole reply must be at most {max_chars} characters. This is a hard limit.\n\
         - Plain text only: no markdown, no HTML, no bullet points.\n\n\
         Title: {title}\n\n\
         Article:\n{text}"
    )
}

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
        text: &'a str,
        max_chars: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let body = json!({
                "model": self.model,
                "messages": [{ "role": "user", "content": prompt(text, max_chars) }],
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

fn prompt(text: &str, max_chars: usize) -> String {
    format!(
        "Summarise this ISRO news article for a notification.\n\
         Reply with:\n\
         1. A title on the first line, at most 80 characters.\n\
         2. One blank line.\n\
         3. A summary of 2-3 complete sentences.\n\
         The whole reply must be at most {max_chars} characters.\n\
         Plain text only: no markdown, no HTML, no bullet points.\n\n\
         Article:\n{text}"
    )
}

use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::json;

const RESOLUTION_SYSTEM_PROMPT: &str =
    "You are an expert compiler and conflict resolver. You will be given the BASE, SIDE A, and \
     SIDE B of a source code file. You must output the fully resolved, syntactically correct \
     file. Do not include markdown blocks (like ```rust). Do not explain your changes. Output \
     ONLY the raw, compilable code.";

/// Model-agnostic AI provider used for semantic conflict resolution.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Resolve a file-level conflict using the three-way merge inputs.
    async fn resolve_conflict(
        &self,
        base: &str,
        side_a: &str,
        side_b: &str,
        file_path: &str,
    ) -> Result<String>;
}

/// Provider implementation for Anthropic's native messages API.
pub struct AnthropicProvider {
    client: reqwest::Client,
    model: String,
    endpoint: String,
    api_key: String,
}

impl AnthropicProvider {
    /// Construct a new Anthropic provider.
    pub fn new(model: String, endpoint: Option<String>, api_key: String) -> Self {
        let endpoint =
            endpoint.unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, model, endpoint, api_key }
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    async fn resolve_conflict(
        &self,
        base: &str,
        side_a: &str,
        side_b: &str,
        file_path: &str,
    ) -> Result<String> {
        let user_prompt = format!(
            "File path: {file_path}\n\nBASE:\n{base}\n\nSIDE A:\n{side_a}\n\nSIDE B:\n{side_b}"
        );

        let response = self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": self.model,
                "max_tokens": 8192,
                "system": RESOLUTION_SYSTEM_PROMPT,
                "messages": [
                    {
                        "role": "user",
                        "content": user_prompt,
                    }
                ]
            }))
            .send()
            .await
            .with_context(|| format!("failed to call Anthropic endpoint {}", self.endpoint))?
            .error_for_status()
            .with_context(|| format!("Anthropic endpoint returned error {}", self.endpoint))?;

        let payload: serde_json::Value =
            response.json().await.context("failed to decode Anthropic response as JSON")?;

        let content = payload["content"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item["text"].as_str())
            .map(str::trim)
            .ok_or_else(|| anyhow::anyhow!("Anthropic response did not contain content[0].text"))?;

        Ok(strip_code_fence(content))
    }
}

/// Provider implementation for OpenAI-compatible chat completions APIs.
pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    model: String,
    endpoint: String,
    api_key: String,
}

impl OpenAiCompatibleProvider {
    /// Construct a provider for OpenAI-compatible endpoints.
    pub fn new(model: String, endpoint: Option<String>, api_key: String) -> Self {
        let endpoint = endpoint
            .unwrap_or_else(|| "https://api.openai.com".to_string())
            .trim_end_matches('/')
            .to_string();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, model, endpoint, api_key }
    }
}

#[async_trait]
impl AiProvider for OpenAiCompatibleProvider {
    async fn resolve_conflict(
        &self,
        base: &str,
        side_a: &str,
        side_b: &str,
        file_path: &str,
    ) -> Result<String> {
        let user_prompt = format!(
            "File path: {file_path}\n\nBASE:\n{base}\n\nSIDE A:\n{side_a}\n\nSIDE B:\n{side_b}"
        );

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": RESOLUTION_SYSTEM_PROMPT,
                    },
                    {
                        "role": "user",
                        "content": user_prompt,
                    }
                ]
            }))
            .send()
            .await
            .with_context(|| {
                format!("failed to call OpenAI-compatible endpoint {}", self.endpoint)
            })?
            .error_for_status()
            .with_context(|| {
                format!("OpenAI-compatible endpoint returned error {}", self.endpoint)
            })?;

        let payload: serde_json::Value =
            response.json().await.context("failed to decode OpenAI-compatible response as JSON")?;

        let content = payload["choices"][0]["message"]["content"]
            .as_str()
            .map(str::trim)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "OpenAI-compatible response did not contain choices[0].message.content"
                )
            })?;

        Ok(strip_code_fence(content))
    }
}

/// Create a boxed provider from config values.
pub fn build_provider(
    provider: &str,
    model: &str,
    endpoint: Option<String>,
    api_key: String,
) -> Result<Box<dyn AiProvider>> {
    match provider {
        "anthropic" => Ok(Box::new(AnthropicProvider::new(model.to_string(), endpoint, api_key))),
        "openai-compatible" => {
            Ok(Box::new(OpenAiCompatibleProvider::new(model.to_string(), endpoint, api_key)))
        }
        other => {
            bail!("unsupported ai.provider '{other}', expected 'anthropic' or 'openai-compatible'")
        }
    }
}

fn strip_code_fence(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    let mut lines = trimmed.lines();
    let _ = lines.next();
    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim_start().starts_with("```") {
            break;
        }
        body.push(line);
    }
    body.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_code_fence_removes_markdown_block() {
        let input = "```rust\nfn main() {}\n```";
        assert_eq!(strip_code_fence(input), "fn main() {}");
    }

    #[test]
    fn strip_code_fence_handles_plain_text() {
        assert_eq!(strip_code_fence("no fence here"), "no fence here");
    }

    #[test]
    fn strip_code_fence_trims_whitespace() {
        let input = "  \n  ```json\n  {\"key\": \"val\"}  \n  ```  \n  ";
        assert_eq!(strip_code_fence(input), "{\"key\": \"val\"}");
    }

    #[test]
    fn anthropic_provider_default_endpoint() {
        let provider = AnthropicProvider::new("claude-3".to_string(), None, "test-key".to_string());
        assert_eq!(provider.endpoint, "https://api.anthropic.com/v1/messages");
        assert_eq!(provider.model, "claude-3");
        assert_eq!(provider.api_key, "test-key");
    }

    #[test]
    fn anthropic_provider_custom_endpoint() {
        let provider = AnthropicProvider::new(
            "claude-3".to_string(),
            Some("https://proxy.example.com/v1".to_string()),
            "key123".to_string(),
        );
        assert_eq!(provider.endpoint, "https://proxy.example.com/v1");
    }

    #[test]
    fn openai_compatible_provider_trims_trailing_slash() {
        let provider = OpenAiCompatibleProvider::new(
            "gpt-4".to_string(),
            Some("https://api.openai.com/".to_string()),
            "key".to_string(),
        );
        assert_eq!(provider.endpoint, "https://api.openai.com");
    }

    #[test]
    fn openai_compatible_provider_default_endpoint() {
        let provider = OpenAiCompatibleProvider::new("gpt-4".to_string(), None, "key".to_string());
        assert_eq!(provider.endpoint, "https://api.openai.com");
    }

    #[test]
    fn build_provider_anthropic() {
        let result = build_provider("anthropic", "claude-3", None, "k".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn build_provider_openai_compatible() {
        let result = build_provider("openai-compatible", "gpt-4", None, "k".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn build_provider_unsupported() {
        let result = build_provider("ollama", "llama", None, "k".to_string());
        assert!(result.is_err());
    }
}

//! Multi-provider AI abstraction layer for thermal curve generation and recommendations.
//!
//! Supports: Anthropic, OpenAI, Google Gemini, Ollama (local), OpenRouter.
//! Factory pattern: `AIProvider::new(provider_name, api_key)` returns a boxed implementation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AIProviderKind {
    Anthropic,
    OpenAI,
    Gemini,
    Ollama,
    OpenRouter,
}

impl AIProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
            Self::OpenRouter => "openrouter",
        }
    }
}

/// Trait for AI provider implementations.
#[async_trait::async_trait]
pub trait AIProvider: Send + Sync {
    /// Send a prompt and get a response.
    async fn send_prompt(&self, prompt: &str) -> anyhow::Result<String>;

    /// Get the provider kind for logging/identification.
    fn kind(&self) -> AIProviderKind;
}

/// Anthropic Claude API provider.
pub struct AnthropicProvider {
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl AIProvider for AnthropicProvider {
    async fn send_prompt(&self, prompt: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": prompt
            }]
        });

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Anthropic request failed: {}", e))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Anthropic API error: {}", text));
        }

        let data: serde_json::Value = response.json().await?;
        let content = data["content"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("text"))
            .and_then(|text| text.as_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid Anthropic response format"))?;

        Ok(content.to_string())
    }

    fn kind(&self) -> AIProviderKind {
        AIProviderKind::Anthropic
    }
}

/// OpenAI ChatGPT API provider.
pub struct OpenAIProvider {
    api_key: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl AIProvider for OpenAIProvider {
    async fn send_prompt(&self, prompt: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "model": "gpt-4",
            "messages": [{
                "role": "user",
                "content": prompt
            }],
            "max_tokens": 1024
        });

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("OpenAI request failed: {}", e))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("OpenAI API error: {}", text));
        }

        let data: serde_json::Value = response.json().await?;
        let content = data["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid OpenAI response format"))?;

        Ok(content.to_string())
    }

    fn kind(&self) -> AIProviderKind {
        AIProviderKind::OpenAI
    }
}

/// Google Gemini API provider.
pub struct GeminiProvider {
    api_key: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl AIProvider for GeminiProvider {
    async fn send_prompt(&self, prompt: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "contents": [{
                "parts": [{
                    "text": prompt
                }]
            }]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={}",
            self.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Gemini request failed: {}", e))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Gemini API error: {}", text));
        }

        let data: serde_json::Value = response.json().await?;
        let content = data["candidates"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(|parts| parts.as_array())
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("text"))
            .and_then(|text| text.as_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid Gemini response format"))?;

        Ok(content.to_string())
    }

    fn kind(&self) -> AIProviderKind {
        AIProviderKind::Gemini
    }
}

/// Ollama local LLM provider (self-hosted).
pub struct OllamaProvider {
    base_url: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl AIProvider for OllamaProvider {
    async fn send_prompt(&self, prompt: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "model": "mistral",
            "prompt": prompt,
            "stream": false
        });

        let url = format!("{}/api/generate", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Ollama request failed: {}", e))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("Ollama API error: {}", text));
        }

        let data: serde_json::Value = response.json().await?;
        let response_text = data["response"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid Ollama response format"))?;

        Ok(response_text.to_string())
    }

    fn kind(&self) -> AIProviderKind {
        AIProviderKind::Ollama
    }
}

/// OpenRouter proxy provider (supports many models).
pub struct OpenRouterProvider {
    api_key: String,
    client: reqwest::Client,
}

impl OpenRouterProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl AIProvider for OpenRouterProvider {
    async fn send_prompt(&self, prompt: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "model": "openai/gpt-4",
            "messages": [{
                "role": "user",
                "content": prompt
            }],
            "max_tokens": 1024
        });

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .header("HTTP-Referer", "https://thermaliq.local")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("OpenRouter request failed: {}", e))?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("OpenRouter API error: {}", text));
        }

        let data: serde_json::Value = response.json().await?;
        let content = data["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid OpenRouter response format"))?;

        Ok(content.to_string())
    }

    fn kind(&self) -> AIProviderKind {
        AIProviderKind::OpenRouter
    }
}

/// Factory function: create an AI provider from kind + api_key.
pub fn create_provider(
    kind: AIProviderKind,
    api_key: String,
) -> anyhow::Result<Box<dyn AIProvider>> {
    match kind {
        AIProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::new(api_key))),
        AIProviderKind::OpenAI => Ok(Box::new(OpenAIProvider::new(api_key))),
        AIProviderKind::Gemini => Ok(Box::new(GeminiProvider::new(api_key))),
        AIProviderKind::Ollama => Ok(Box::new(OllamaProvider::new(api_key))),
        AIProviderKind::OpenRouter => Ok(Box::new(OpenRouterProvider::new(api_key))),
    }
}

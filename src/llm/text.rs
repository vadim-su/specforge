use anyhow::{Context, Result, bail};
use rig::{
    client::{CompletionClient, ProviderClient},
    completion::{AssistantContent, CompletionModel, CompletionResponse},
    providers::{anthropic, chatgpt, ollama, openai},
};
use serde::de::DeserializeOwned;

use crate::provider::{Provider, default_model};

#[derive(Debug)]
pub struct LlmPrompt {
    pub system: String,
    pub user: String,
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    provider: Provider,
    model: String,
}

impl LlmClient {
    pub fn new(provider: Provider, model: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| default_model(provider).to_string());

        Self { provider, model }
    }

    pub async fn complete(&self, prompt: LlmPrompt) -> Result<String> {
        match self.provider {
            Provider::Openai => {
                let client = openai::Client::from_env().context(
                    "failed to initialize OpenAI client; set OPENAI_API_KEY or choose another provider",
                )?;
                complete_with_model(client.completion_model(&self.model), prompt).await
            }
            Provider::Anthropic => {
                let client = anthropic::Client::from_env().context(
                    "failed to initialize Anthropic client; set ANTHROPIC_API_KEY or choose another provider",
                )?;
                complete_with_model(client.completion_model(&self.model), prompt).await
            }
            Provider::Ollama => {
                let client = ollama::Client::from_env().context(
                    "failed to initialize Ollama client; set OLLAMA_API_BASE_URL if not using localhost",
                )?;
                complete_with_model(client.completion_model(&self.model), prompt).await
            }
            Provider::Chatgpt => {
                let client = chatgpt::Client::from_env().context(
                    "failed to initialize ChatGPT client; set CHATGPT_ACCESS_TOKEN or complete OAuth",
                )?;
                complete_with_model(client.completion_model(&self.model), prompt).await
            }
        }
    }
}

async fn complete_with_model<M>(model: M, prompt: LlmPrompt) -> Result<String>
where
    M: CompletionModel,
    M::Response: DeserializeOwned,
{
    let request = model
        .completion_request(prompt.user)
        .preamble(prompt.system)
        .temperature_opt(prompt.temperature);

    let response = request
        .send()
        .await
        .context("LLM completion request failed")?;
    completion_response_text(response)
}

fn completion_response_text<T>(response: CompletionResponse<T>) -> Result<String>
where
    T: DeserializeOwned,
{
    let text = response
        .choice
        .into_iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if text.is_empty() {
        bail!("LLM returned no assistant text");
    }

    Ok(text)
}

use anyhow::{Context, Result};
use rig::{
    client::{CompletionClient, ProviderClient},
    providers::{anthropic, chatgpt, ollama, openai},
};

use crate::provider::{Provider, default_model};

use super::{
    agent::{RuntimeAgent, build_agent},
    config::RigAgentConfig,
};

#[derive(Debug, Clone)]
pub struct RigAgentFactory {
    provider: Provider,
    model: String,
}

impl RigAgentFactory {
    pub fn new(provider: Provider, model: Option<String>) -> Self {
        let model = model.unwrap_or_else(|| default_model(provider).to_string());

        Self { provider, model }
    }

    pub fn build(&self, config: RigAgentConfig) -> Result<RuntimeAgent> {
        match self.provider {
            Provider::Openai => {
                let client = openai::Client::from_env().context(
                    "failed to initialize OpenAI client; set OPENAI_API_KEY or choose another provider",
                )?;
                Ok(RuntimeAgent::Openai(build_agent(
                    client.agent(&self.model),
                    config,
                )))
            }
            Provider::Anthropic => {
                let client = anthropic::Client::from_env().context(
                    "failed to initialize Anthropic client; set ANTHROPIC_API_KEY or choose another provider",
                )?;
                Ok(RuntimeAgent::Anthropic(build_agent(
                    client.agent(&self.model),
                    config,
                )))
            }
            Provider::Ollama => {
                let client = ollama::Client::from_env().context(
                    "failed to initialize Ollama client; set OLLAMA_API_BASE_URL if not using localhost",
                )?;
                Ok(RuntimeAgent::Ollama(build_agent(
                    client.agent(&self.model),
                    config,
                )))
            }
            Provider::Chatgpt => {
                let client = chatgpt::Client::from_env().context(
                    "failed to initialize ChatGPT client; set CHATGPT_ACCESS_TOKEN or complete OAuth",
                )?;
                Ok(RuntimeAgent::Chatgpt(build_agent(
                    client.agent(&self.model),
                    config,
                )))
            }
        }
    }
}

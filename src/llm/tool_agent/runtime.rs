use anyhow::{Context, Result, bail};
use rig::{
    agent::{Agent, AgentBuilder},
    completion::{Completion, CompletionModel},
    message::Message,
    providers::{anthropic, chatgpt, ollama, openai},
};
use serde::de::DeserializeOwned;

use super::{
    config::{RigAgentConfig, RigAgentTurn},
    response::{assistant_text, assistant_tool_calls},
};

type OpenAiAgent = Agent<openai::responses_api::ResponsesCompletionModel>;
type AnthropicAgent = Agent<anthropic::completion::CompletionModel>;
type OllamaAgent = Agent<ollama::CompletionModel>;
type ChatGptAgent = Agent<chatgpt::ResponsesCompletionModel>;

pub enum RuntimeAgent {
    Openai(OpenAiAgent),
    Anthropic(AnthropicAgent),
    Ollama(OllamaAgent),
    Chatgpt(ChatGptAgent),
}

impl RuntimeAgent {
    pub async fn turn(&self, prompt: Message, history: Vec<Message>) -> Result<RigAgentTurn> {
        match self {
            Self::Openai(agent) => complete_turn(agent, prompt, history).await,
            Self::Anthropic(agent) => complete_turn(agent, prompt, history).await,
            Self::Ollama(agent) => complete_turn(agent, prompt, history).await,
            Self::Chatgpt(agent) => complete_turn(agent, prompt, history).await,
        }
    }
}

pub(super) fn build_agent<M>(builder: AgentBuilder<M>, config: RigAgentConfig) -> Agent<M>
where
    M: CompletionModel + 'static,
{
    let mut builder = builder.name(&config.name).preamble(&config.preamble);
    if let Some(temperature) = config.temperature {
        builder = builder.temperature(temperature);
    }
    if let Some(max_tokens) = config.max_tokens {
        builder = builder.max_tokens(max_tokens);
    }

    if let Some(handle) = config.tool_server_handle {
        builder.tool_server_handle(handle).build()
    } else {
        builder.tools(config.tools).build()
    }
}

async fn complete_turn<M>(
    agent: &Agent<M>,
    prompt: Message,
    history: Vec<Message>,
) -> Result<RigAgentTurn>
where
    M: CompletionModel,
    M::Response: DeserializeOwned,
{
    let response = agent
        .completion(prompt, history)
        .await?
        .send()
        .await
        .context("Rig agent completion request failed")?;

    let text = assistant_text(&response.choice);
    let tool_calls = assistant_tool_calls(&response.choice);
    if text.trim().is_empty() && tool_calls.is_empty() {
        bail!("Rig agent returned neither assistant text nor tool calls");
    }

    let assistant_message = Message::Assistant {
        id: response.message_id.clone(),
        content: response.choice,
    };

    Ok(RigAgentTurn {
        assistant_message,
        text,
        tool_calls,
    })
}

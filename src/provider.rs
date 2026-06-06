use rig::providers::openai;

#[derive(Debug, Clone, Copy)]
pub enum Provider {
    Openai,
    Anthropic,
    Ollama,
    Chatgpt,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Provider::Openai => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Ollama => "ollama",
            Provider::Chatgpt => "chatgpt",
        };

        formatter.write_str(value)
    }
}

impl std::str::FromStr for Provider {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "openai" => Ok(Self::Openai),
            "anthropic" => Ok(Self::Anthropic),
            "ollama" => Ok(Self::Ollama),
            "chatgpt" => Ok(Self::Chatgpt),
            _ => Err(format!(
                "unsupported provider `{value}`; expected one of: openai, anthropic, ollama, chatgpt"
            )),
        }
    }
}

pub fn default_model(provider: Provider) -> &'static str {
    match provider {
        Provider::Openai => openai::GPT_5_NANO,
        Provider::Anthropic => "claude-sonnet-4-6",
        Provider::Ollama => "llama3.2",
        Provider::Chatgpt => "gpt-5.5",
    }
}

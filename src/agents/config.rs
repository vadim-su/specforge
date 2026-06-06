use rig::{message::Message, tool::ToolDyn};

pub struct RigAgentConfig {
    pub name: String,
    pub preamble: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub tools: Vec<Box<dyn ToolDyn>>,
}

pub struct RigAgentTurn {
    pub assistant_message: Message,
    pub text: String,
    pub tool_calls: Vec<rig::message::ToolCall>,
}

use std::{collections::BTreeMap, process::Stdio};

use anyhow::{Context, Result};
use rig::tool::{rmcp::McpClientHandler, server::ToolServerHandle};
use rmcp::{
    model::{ClientCapabilities, ClientInfo, Implementation},
    transport::TokioChildProcess,
};
use tokio::process::Command;

use crate::config::{ProjectIntegrationsConfig, ProjectMcpServerConfig};

pub struct McpRuntime {
    _services: Vec<rmcp::service::RunningService<rmcp::RoleClient, McpClientHandler>>,
}

impl McpRuntime {
    pub async fn start(
        integrations: &ProjectIntegrationsConfig,
        tool_server_handle: ToolServerHandle,
    ) -> Result<Self> {
        let mut services = Vec::new();
        for (server_id, config) in &integrations.mcp {
            if !config.enabled {
                continue;
            }

            let transport = stdio_transport(server_id, config)?;
            let client_info = ClientInfo::new(
                ClientCapabilities::default(),
                Implementation::new("specforge", env!("CARGO_PKG_VERSION")),
            );
            let handler = McpClientHandler::new(client_info, tool_server_handle.clone());
            let service = handler
                .connect(transport)
                .await
                .with_context(|| format!("failed to connect MCP server `{server_id}`"))?;
            services.push(service);
        }

        Ok(Self {
            _services: services,
        })
    }

    pub fn is_empty(&self) -> bool {
        self._services.is_empty()
    }
}

fn stdio_transport(server_id: &str, config: &ProjectMcpServerConfig) -> Result<TokioChildProcess> {
    let mut command = Command::new(&config.command);
    command.args(&config.args);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::inherit());
    apply_env(&mut command, &config.env_vars, &config.env);

    TokioChildProcess::new(command)
        .with_context(|| format!("failed to start MCP server `{server_id}`"))
}

fn apply_env(command: &mut Command, inherited: &[String], inline: &BTreeMap<String, String>) {
    command.env_clear();
    for name in BASELINE_ENV_VARS {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    for name in inherited {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    for (name, value) in inline {
        command.env(name, value);
    }
}

const BASELINE_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SystemRoot",
    "ComSpec",
];

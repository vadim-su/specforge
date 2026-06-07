use std::path::PathBuf;

use clap::{Parser, Subcommand};
use specforge::{config::DEFAULT_SPEC, provider::Provider};

use crate::cli::color::ColorMode;

#[derive(Debug, Parser)]
#[command(version, about = "Spec-driven project state tooling")]
pub struct Cli {
    /// Project root used for spec paths, .specforge state, and agent file tools.
    #[arg(long, global = true, value_name = "DIR")]
    pub project_root: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize canonical spec.adoc from prose using an LLM.
    Init {
        /// Plain-text/Markdown idea file. If omitted, init reads stdin when piped.
        input: Option<PathBuf>,
        #[arg(short, long, default_value = DEFAULT_SPEC)]
        output: PathBuf,
        /// Overwrite existing spec/state files.
        #[arg(long)]
        force: bool,
        /// Create a deterministic starter template instead of calling an LLM.
        #[arg(long)]
        template: bool,
        #[arg(long, default_value_t = Provider::Openai)]
        provider: Provider,
        /// Provider model name. Defaults depend on --provider.
        #[arg(long)]
        model: Option<String>,
        /// Disable the ratatui init questionnaire.
        #[arg(long)]
        no_tui: bool,
    },
    /// Validate a restricted AsciiDoc spec.
    Check {
        #[arg(default_value = DEFAULT_SPEC)]
        spec: PathBuf,
    },
    /// Print the normalized spec model as JSON.
    Model {
        #[arg(default_value = DEFAULT_SPEC)]
        spec: PathBuf,
    },
    /// Compare the spec with the stored current state.
    Diff {
        #[arg(default_value = DEFAULT_SPEC)]
        spec: PathBuf,
        #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
        color: ColorMode,
    },
    /// Normalize spec tags, show diff, update current state, and start the codegen agent.
    Sync {
        #[arg(default_value = DEFAULT_SPEC)]
        spec: PathBuf,
        /// Accept without interactive confirmation.
        #[arg(short, long)]
        yes: bool,
        /// Accept the spec without starting the codegen agent.
        #[arg(long)]
        skip_agent: bool,
        /// Maximum LLM/tool turns for the codegen agent. Use 0 for no turn budget.
        #[arg(long)]
        agent_steps: Option<usize>,
        #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
        color: ColorMode,
        #[arg(long, default_value_t = Provider::Openai)]
        provider: Provider,
        /// Provider model name. Defaults depend on --provider.
        #[arg(long)]
        model: Option<String>,
        /// Disable the ratatui progress view.
        #[arg(long)]
        no_tui: bool,
    },
    /// Apply an ad-hoc code fix or update that is not part of the spec.
    Fix {
        /// Change request. If omitted, fix reads stdin when piped.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        request: Vec<String>,
        /// Maximum LLM/tool turns for the code change agent. Use 0 for no turn budget.
        #[arg(long)]
        agent_steps: Option<usize>,
        #[arg(long, default_value_t = Provider::Openai)]
        provider: Provider,
        /// Provider model name. Defaults depend on --provider.
        #[arg(long)]
        model: Option<String>,
        /// Disable the ratatui progress view.
        #[arg(long)]
        no_tui: bool,
    },
    /// Ask an LLM for questions that can improve a spec and project direction.
    Assist {
        #[command(subcommand)]
        command: AssistCommand,
    },
    /// Store the spec as the current state for future diffs.
    Accept {
        #[arg(default_value = DEFAULT_SPEC)]
        spec: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum AssistCommand {
    /// Expand a spec by asking targeted product and implementation questions.
    Expand {
        #[arg(default_value = DEFAULT_SPEC)]
        spec: PathBuf,
        /// Optional area to focus the questions on.
        #[arg(long)]
        focus: Option<String>,
        #[arg(long, default_value_t = Provider::Openai)]
        provider: Provider,
        /// Provider model name. Defaults depend on --provider.
        #[arg(long)]
        model: Option<String>,
        /// Print generated questions instead of opening the ratatui questionnaire.
        #[arg(long)]
        no_tui: bool,
    },
}

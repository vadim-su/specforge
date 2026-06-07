use anyhow::{Result, bail};

use crate::{
    llm::{LlmClient, LlmPrompt, strip_code_fence},
    prompts,
    provider::Provider,
    spec::{ParsedSpec, Severity, needs_tag_normalization, parse_spec, validate_model},
};

#[derive(Debug)]
pub struct SyncTagOptions {
    pub provider: Provider,
    pub model: Option<String>,
}

pub async fn normalize_spec_tags(
    baseline: &ParsedSpec,
    current: &ParsedSpec,
    options: SyncTagOptions,
) -> Result<ParsedSpec> {
    let diagnostics = validate_model(&current.model);
    if diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != Severity::Error)
        && !needs_tag_normalization(&current.model)
    {
        return Ok(current.clone());
    }

    let client = LlmClient::new(options.provider, options.model);
    let generated = client
        .complete(LlmPrompt {
            system: prompts::SYNC_SPEC_SYSTEM.to_string(),
            user: sync_user_prompt(&baseline.source, &current.source),
            temperature: Some(0.1),
        })
        .await?;
    let source = strip_code_fence(&generated);
    let model = parse_spec(&source);

    let diagnostics = validate_model(&model);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        bail!("LLM-normalized spec is still invalid; no files were written");
    }
    if needs_tag_normalization(&model) {
        bail!("LLM-normalized spec still has untagged sections; no files were written");
    }

    Ok(ParsedSpec { source, model })
}

fn sync_user_prompt(baseline: &str, current: &str) -> String {
    format!(
        "Normalize anchors and IDs in the edited SpecForge spec using the stored current spec for ID continuity. If the stored current spec is empty, assign stable IDs as an initial spec.\n\n<stored-current-spec>\n{}\n</stored-current-spec>\n\n<edited-spec>\n{}\n</edited-spec>\n",
        baseline.trim(),
        current.trim()
    )
}

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct ParsedSpec {
    pub source: String,
    pub model: SpecModel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecModel {
    pub spec_version: u32,
    pub document: DocumentInfo,
    pub items: Vec<SpecItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentInfo {
    pub title: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecItem {
    pub id: Option<String>,
    pub kind: SpecKind,
    pub title: String,
    pub heading: String,
    pub level: usize,
    pub metadata: BTreeMap<String, String>,
    pub content_hash: String,
    pub source_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpecKind {
    Project,
    Feature,
    Entity,
    Command,
    Flow,
    Acceptance,
    Constraint,
    Decision,
    Glossary,
    Term,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub line: usize,
    pub message: String,
}

#[derive(Debug)]
struct OpenSection {
    id: Option<String>,
    heading: String,
    level: usize,
    start_line: usize,
    content_start: usize,
}

pub fn parse_spec_file(path: &Path) -> Result<ParsedSpec> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let model = parse_spec(&source);

    Ok(ParsedSpec { source, model })
}

pub fn parse_spec(source: &str) -> SpecModel {
    let lines: Vec<&str> = source.lines().collect();
    let mut title = None;
    let mut attributes = BTreeMap::new();
    let mut items = Vec::new();
    let mut pending_anchor: Option<String> = None;
    let mut open: Option<OpenSection> = None;

    for (idx, line) in lines.iter().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();

        if title.is_none() {
            if let Some(rest) = trimmed.strip_prefix("= ") {
                title = Some(rest.trim().to_string());
                continue;
            }
        }

        if open.is_none() {
            if let Some((name, value)) = parse_attribute(trimmed) {
                attributes.insert(name, value);
                continue;
            }
        }

        if let Some(anchor) = parse_anchor(trimmed) {
            pending_anchor = Some(anchor);
            continue;
        }

        if let Some((level, heading)) = parse_heading(trimmed) {
            if level > 1 {
                if let Some(section) = open.take() {
                    items.push(build_item(section, &lines));
                }

                open = Some(OpenSection {
                    id: pending_anchor.take(),
                    heading,
                    level,
                    start_line: line_number,
                    content_start: line_number + 1,
                });
            }
        }
    }

    if let Some(section) = open {
        items.push(build_item(section, &lines));
    }

    SpecModel {
        spec_version: 1,
        document: DocumentInfo {
            title,
            attributes,
            content_hash: hash_text(source),
        },
        items,
    }
}

pub fn validate_model(model: &SpecModel) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();

    for item in &model.items {
        if heading_has_kind_prefix(&item.heading) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                line: item.source_range.start_line,
                message: format!(
                    "heading `{}` uses a visible kind prefix; use the anchor prefix instead",
                    item.heading
                ),
            });
        }

        if item.kind != SpecKind::Unknown && item.kind != SpecKind::Project && item.id.is_none() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                line: item.source_range.start_line,
                message: format!("{} section must have an anchor id", item.heading),
            });
        }

        if let Some(id) = &item.id {
            if let Some(first_line) = ids.get(id.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    line: item.source_range.start_line,
                    message: format!("duplicate id `{id}`; first seen at line {first_line}"),
                });
            } else {
                ids.insert(id, item.source_range.start_line);
            }

            if let Some(expected_prefix) = expected_prefix(&item.kind) {
                if !id_matches_kind(id, &item.kind) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        line: item.source_range.start_line,
                        message: format!(
                            "id `{id}` should start with `{expected_prefix}` for {:?}",
                            item.kind
                        ),
                    });
                }
            }
        }

        if item.kind == SpecKind::Unknown && item.id.is_some() {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                line: item.source_range.start_line,
                message: format!(
                    "anchored section `{}` has no recognized type prefix",
                    item.heading
                ),
            });
        }
    }

    diagnostics
}

pub fn needs_tag_normalization(model: &SpecModel) -> bool {
    model
        .items
        .iter()
        .any(|item| item.kind == SpecKind::Unknown && item.id.is_none())
}

pub fn print_diagnostics(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        println!(
            "{severity}: line {}: {}",
            diagnostic.line, diagnostic.message
        );
    }
}

fn parse_attribute(trimmed: &str) -> Option<(String, String)> {
    let rest = trimmed.strip_prefix(':')?;
    let (name, value) = rest.split_once(':')?;

    if name.is_empty() {
        return None;
    }

    Some((name.trim().to_string(), value.trim().to_string()))
}

fn parse_anchor(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("[[")?;
    let id = rest.strip_suffix("]]")?;

    if id.trim().is_empty() {
        return None;
    }

    Some(id.trim().to_string())
}

fn parse_heading(trimmed: &str) -> Option<(usize, String)> {
    let level = trimmed.chars().take_while(|ch| *ch == '=').count();
    if level == 0 || !trimmed.chars().nth(level).is_some_and(|ch| ch == ' ') {
        return None;
    }

    Some((level, trimmed[level + 1..].trim().to_string()))
}

fn build_item(section: OpenSection, lines: &[&str]) -> SpecItem {
    let end_line = find_section_end(section.content_start, lines);
    let content = if section.content_start <= end_line {
        lines[(section.content_start - 1)..end_line].join("\n")
    } else {
        String::new()
    };
    let (kind, title) = parse_kind_and_title(section.id.as_deref(), &section.heading);

    SpecItem {
        id: section.id,
        kind,
        title,
        heading: section.heading,
        level: section.level,
        metadata: parse_metadata(&content),
        content_hash: hash_text(&normalize_content(&content)),
        source_range: SourceRange {
            start_line: section.start_line,
            end_line,
        },
    }
}

fn find_section_end(content_start: usize, lines: &[&str]) -> usize {
    if content_start == 0 || content_start > lines.len() {
        return content_start.saturating_sub(1);
    }

    for (idx, line) in lines.iter().enumerate().skip(content_start - 1) {
        if parse_heading(line.trim()).is_some() {
            return idx;
        }
    }

    lines.len()
}

fn parse_kind_and_title(id: Option<&str>, heading: &str) -> (SpecKind, String) {
    if heading.trim().eq_ignore_ascii_case("project") {
        return (SpecKind::Project, "Project".to_string());
    }

    if let Some(kind) = id.and_then(kind_from_anchor_id) {
        return (kind, heading.trim().to_string());
    }

    (SpecKind::Unknown, heading.trim().to_string())
}

fn heading_has_kind_prefix(heading: &str) -> bool {
    let Some((raw_kind, _)) = heading.split_once(':') else {
        return false;
    };

    matches!(
        raw_kind.trim().to_ascii_lowercase().as_str(),
        "feature"
            | "entity"
            | "command"
            | "flow"
            | "acceptance"
            | "constraint"
            | "decision"
            | "glossary"
            | "term"
    )
}

fn kind_from_anchor_id(id: &str) -> Option<SpecKind> {
    if id.starts_with("feat.") {
        Some(SpecKind::Feature)
    } else if id.starts_with("entity.") {
        Some(SpecKind::Entity)
    } else if id.starts_with("cmd.") {
        Some(SpecKind::Command)
    } else if id.starts_with("flow.") {
        Some(SpecKind::Flow)
    } else if id.starts_with("acc.") {
        Some(SpecKind::Acceptance)
    } else if id.starts_with("constraint.") {
        Some(SpecKind::Constraint)
    } else if id.starts_with("decision.") {
        Some(SpecKind::Decision)
    } else if id == "glossary" || id.starts_with("glossary.") {
        Some(SpecKind::Glossary)
    } else if id.starts_with("term.") {
        Some(SpecKind::Term)
    } else {
        None
    }
}

fn parse_metadata(content: &str) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let Some((key, value)) = trimmed.split_once("::") else {
            continue;
        };

        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == ' ' || ch == '-' || ch == '_')
        {
            continue;
        }

        metadata.insert(normalize_key(key), value.trim().to_string());
    }

    metadata
}

fn normalize_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_content(content: &str) -> String {
    content
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn hash_text(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    format!("sha256:{digest:x}")
}

fn expected_prefix(kind: &SpecKind) -> Option<&'static str> {
    match kind {
        SpecKind::Project => None,
        SpecKind::Feature => Some("feat."),
        SpecKind::Entity => Some("entity."),
        SpecKind::Command => Some("cmd."),
        SpecKind::Flow => Some("flow."),
        SpecKind::Acceptance => Some("acc."),
        SpecKind::Constraint => Some("constraint."),
        SpecKind::Decision => Some("decision."),
        SpecKind::Glossary => Some("glossary or glossary."),
        SpecKind::Term => Some("term."),
        SpecKind::Unknown => None,
    }
}

fn id_matches_kind(id: &str, kind: &SpecKind) -> bool {
    match kind {
        SpecKind::Project => true,
        SpecKind::Feature => id.starts_with("feat."),
        SpecKind::Entity => id.starts_with("entity."),
        SpecKind::Command => id.starts_with("cmd."),
        SpecKind::Flow => id.starts_with("flow."),
        SpecKind::Acceptance => id.starts_with("acc."),
        SpecKind::Constraint => id.starts_with("constraint."),
        SpecKind::Decision => id.starts_with("decision."),
        SpecKind::Glossary => id == "glossary" || id.starts_with("glossary."),
        SpecKind::Term => id.starts_with("term."),
        SpecKind::Unknown => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typed_sections_with_anchor_ids() {
        let model = parse_spec(
            r#"= Test Spec
:project-id: test

[[feat.customer-management]]
== Customer management

Status:: planned
Priority:: high

The user can manage customers.
"#,
        );

        assert_eq!(model.document.title.as_deref(), Some("Test Spec"));
        assert_eq!(model.items.len(), 1);

        let item = &model.items[0];
        assert_eq!(item.id.as_deref(), Some("feat.customer-management"));
        assert_eq!(item.kind, SpecKind::Feature);
        assert_eq!(item.title, "Customer management");
        assert_eq!(
            item.metadata.get("status").map(String::as_str),
            Some("planned")
        );
        assert_eq!(
            item.metadata.get("priority").map(String::as_str),
            Some("high")
        );
    }

    #[test]
    fn rejects_visible_kind_prefixes() {
        let model = parse_spec(
            r#"= Test Spec

== Feature: Customer management

The user can manage customers.
"#,
        );

        let diagnostics = validate_model(&model);

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.message.contains("visible kind prefix")
        }));
    }

    #[test]
    fn rejects_visible_kind_prefixes_even_with_matching_anchor() {
        let model = parse_spec(
            r#"= Test Spec

[[acc.login-with-email]]
==== Acceptance: Login with email
"#,
        );

        let diagnostics = validate_model(&model);

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.message.contains("visible kind prefix")
        }));
    }

    #[test]
    fn infers_kind_from_anchor_prefix_without_heading_prefix() {
        let model = parse_spec(
            r#"= Test Spec

[[feat.customer-management]]
== Customer management

[[entity.customer]]
== Customer

[[glossary]]
== Glossary

[[term.customer]]
=== Customer
"#,
        );

        assert_eq!(model.items[0].kind, SpecKind::Feature);
        assert_eq!(model.items[0].title, "Customer management");
        assert_eq!(model.items[1].kind, SpecKind::Entity);
        assert_eq!(model.items[1].title, "Customer");
        assert_eq!(model.items[2].kind, SpecKind::Glossary);
        assert_eq!(model.items[3].kind, SpecKind::Term);
        assert!(validate_model(&model).is_empty());
    }

    #[test]
    fn detects_plain_sections_that_need_tag_normalization() {
        let model = parse_spec(
            r#"= Test Spec

== Project

== Glossary

A project term.
"#,
        );

        assert!(needs_tag_normalization(&model));
    }
}

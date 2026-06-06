use std::collections::{BTreeMap, BTreeSet};

use crate::spec::{ParsedSpec, SpecItem, SpecKind, SpecModel};

#[derive(Debug)]
pub struct ModelDiff {
    pub added: Vec<SpecItem>,
    pub removed: Vec<SpecItem>,
    pub changed: Vec<ItemChange>,
}

#[derive(Debug)]
pub struct ItemChange {
    pub id: String,
    pub kind: SpecKind,
    pub title: String,
    pub line: usize,
    pub fields: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineDiff {
    Equal(String),
    Added(String),
    Removed(String),
}

pub fn diff_models(accepted: &SpecModel, current: &SpecModel) -> ModelDiff {
    let accepted_by_id = items_by_key(accepted);
    let current_by_id = items_by_key(current);
    let accepted_ids = accepted_by_id.keys().cloned().collect::<BTreeSet<_>>();
    let current_ids = current_by_id.keys().cloned().collect::<BTreeSet<_>>();

    let added = current_ids
        .difference(&accepted_ids)
        .filter_map(|id| current_by_id.get(id).copied().cloned())
        .collect::<Vec<_>>();
    let removed = accepted_ids
        .difference(&current_ids)
        .filter_map(|id| accepted_by_id.get(id).copied().cloned())
        .collect::<Vec<_>>();
    let changed = accepted_ids
        .intersection(&current_ids)
        .filter_map(|id| {
            let before = accepted_by_id.get(id)?;
            let after = current_by_id.get(id)?;
            changed_fields(before, after).map(|fields| ItemChange {
                id: id.to_string(),
                kind: after.kind.clone(),
                title: after.title.clone(),
                line: after.source_range.start_line,
                fields,
            })
        })
        .collect::<Vec<_>>();

    ModelDiff {
        added,
        removed,
        changed,
    }
}

pub fn locate_diff_changes(
    accepted: &ParsedSpec,
    current: &ParsedSpec,
    mut diff: ModelDiff,
) -> ModelDiff {
    let accepted_items = items_by_key(&accepted.model);
    let current_items = items_by_key(&current.model);
    let accepted_lines = source_lines(&accepted.source);
    let current_lines = source_lines(&current.source);

    for change in &mut diff.changed {
        let Some(before) = accepted_items.get(&change.id) else {
            continue;
        };
        let Some(after) = current_items.get(&change.id) else {
            continue;
        };

        let old_lines = section_lines(&accepted_lines, before);
        let new_lines = section_lines(&current_lines, after);
        let line_diff = diff_lines(&old_lines, &new_lines);

        change.line = first_changed_line_from_diff(
            &line_diff,
            before.source_range.start_line,
            after.source_range.start_line,
        )
        .unwrap_or(after.source_range.start_line);
    }

    diff.added.sort_by_key(|item| item.source_range.start_line);
    diff.removed
        .sort_by_key(|item| item.source_range.start_line);
    diff.changed.sort_by_key(|change| change.line);

    diff
}

pub fn items_by_key(model: &SpecModel) -> BTreeMap<String, &SpecItem> {
    model
        .items
        .iter()
        .filter_map(|item| item_key(item).map(|key| (key, item)))
        .collect()
}

fn item_key(item: &SpecItem) -> Option<String> {
    if let Some(id) = &item.id {
        return Some(id.clone());
    }

    match item.kind {
        SpecKind::Project => Some("project".to_string()),
        _ => None,
    }
}

fn changed_fields(before: &SpecItem, after: &SpecItem) -> Option<Vec<&'static str>> {
    let mut fields = Vec::new();

    if before.kind != after.kind {
        fields.push("kind");
    }
    if before.title != after.title {
        fields.push("title");
    }
    if before.level != after.level {
        fields.push("level");
    }
    if before.metadata != after.metadata {
        fields.push("metadata");
    }
    if before.content_hash != after.content_hash {
        fields.push("content");
    }

    (!fields.is_empty()).then_some(fields)
}

pub fn source_lines(source: &str) -> Vec<&str> {
    source.lines().collect()
}

pub fn section_lines(lines: &[&str], item: &SpecItem) -> Vec<String> {
    if item.source_range.start_line == 0
        || item.source_range.end_line < item.source_range.start_line
    {
        return Vec::new();
    }

    let start = item.source_range.start_line.saturating_sub(1);
    let end = item.source_range.end_line.min(lines.len());

    lines[start..end]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

pub fn section_len(item: &SpecItem) -> usize {
    item.source_range
        .end_line
        .saturating_sub(item.source_range.start_line)
        + 1
}

pub fn section_label(item: &SpecItem) -> String {
    if item.kind == SpecKind::Project {
        return "project".to_string();
    }

    format!("{} {}", display_item_key(item), item.heading)
}

pub fn diff_lines(old: &[String], new: &[String]) -> Vec<LineDiff> {
    let mut lengths = vec![vec![0; new.len() + 1]; old.len() + 1];

    for old_idx in 0..old.len() {
        for new_idx in 0..new.len() {
            lengths[old_idx + 1][new_idx + 1] = if old[old_idx] == new[new_idx] {
                lengths[old_idx][new_idx] + 1
            } else {
                lengths[old_idx][new_idx + 1].max(lengths[old_idx + 1][new_idx])
            };
        }
    }

    let mut old_idx = old.len();
    let mut new_idx = new.len();
    let mut diff = Vec::new();

    while old_idx > 0 || new_idx > 0 {
        if old_idx > 0 && new_idx > 0 && old[old_idx - 1] == new[new_idx - 1] {
            diff.push(LineDiff::Equal(old[old_idx - 1].clone()));
            old_idx -= 1;
            new_idx -= 1;
        } else if new_idx > 0
            && (old_idx == 0 || lengths[old_idx][new_idx - 1] >= lengths[old_idx - 1][new_idx])
        {
            diff.push(LineDiff::Added(new[new_idx - 1].clone()));
            new_idx -= 1;
        } else if old_idx > 0 {
            diff.push(LineDiff::Removed(old[old_idx - 1].clone()));
            old_idx -= 1;
        }
    }

    diff.reverse();
    diff
}

fn first_changed_line_from_diff(
    diff: &[LineDiff],
    old_start_line: usize,
    new_start_line: usize,
) -> Option<usize> {
    let mut old_line = old_start_line;
    let mut new_line = new_start_line;

    for line in diff {
        match line {
            LineDiff::Equal(_) => {
                old_line += 1;
                new_line += 1;
            }
            LineDiff::Removed(_) => {
                return Some(old_line);
            }
            LineDiff::Added(_) => {
                return Some(new_line);
            }
        }
    }

    None
}

pub fn visible_diff_lines(diff: &[LineDiff], context: usize) -> BTreeSet<usize> {
    let mut visible = BTreeSet::new();

    for (idx, line) in diff.iter().enumerate() {
        if matches!(line, LineDiff::Added(_) | LineDiff::Removed(_)) {
            let start = idx.saturating_sub(context);
            let end = (idx + context + 1).min(diff.len());

            for visible_idx in start..end {
                visible.insert(visible_idx);
            }
        }
    }

    visible
}

pub fn display_item_key(item: &SpecItem) -> String {
    item.id.clone().unwrap_or_else(|| {
        if item.kind == SpecKind::Project {
            "project".to_string()
        } else {
            "<missing-id>".to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ParsedSpec, parse_spec};

    #[test]
    fn diffs_items_by_id_not_source_range() {
        let accepted = parse_spec(
            r#"= Test Spec

[[feat.customer-management]]
== Feature: Customer management

The user can manage customers.
"#,
        );
        let current = parse_spec(
            r#"= Test Spec

== Notes

Some intro text.

[[feat.customer-management]]
== Feature: Client management

The user can manage customers.
"#,
        );

        let diff = diff_models(&accepted, &current);

        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].id, "feat.customer-management");
        assert_eq!(diff.changed[0].fields, vec!["title"]);
    }

    #[test]
    fn diffs_project_singleton_without_anchor_id() {
        let accepted = parse_spec(
            r#"= Test Spec

== Project

Name:: Todo App
Language:: Rust
"#,
        );
        let current = parse_spec(
            r#"= Test Spec

== Project

Name:: Todo App
Language:: Rust1
"#,
        );

        let diff = diff_models(&accepted, &current);

        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].id, "project");
        assert_eq!(diff.changed[0].fields, vec!["metadata", "content"]);
    }

    #[test]
    fn locates_changed_lines_and_sorts_by_position() {
        let accepted = parsed(
            r#"= Test Spec

== Project

Language:: Rust

[[entity.task]]
== Entity: Task

| title
| string
| Human-readable task title.
"#,
        );
        let current = parsed(
            r#"= Test Spec

== Project

Language:: Rust1

[[entity.task]]
== Entity: Task

| title
| string
| Human-readable task title.1
"#,
        );

        let diff = locate_diff_changes(
            &accepted,
            &current,
            diff_models(&accepted.model, &current.model),
        );

        assert_eq!(diff.changed.len(), 2);
        assert_eq!(diff.changed[0].id, "project");
        assert_eq!(diff.changed[0].line, 5);
        assert_eq!(diff.changed[1].id, "entity.task");
        assert_eq!(diff.changed[1].line, 12);
    }

    #[test]
    fn builds_line_diff_with_context_additions_and_removals() {
        let old = vec![
            "Name:: Todo App".to_string(),
            "Language:: Rust".to_string(),
            "Status:: planned".to_string(),
        ];
        let new = vec![
            "Name:: Todo App".to_string(),
            "Language:: Rust1".to_string(),
            "Status:: planned".to_string(),
        ];

        let diff = diff_lines(&old, &new);

        assert_eq!(
            diff,
            vec![
                LineDiff::Equal("Name:: Todo App".to_string()),
                LineDiff::Removed("Language:: Rust".to_string()),
                LineDiff::Added("Language:: Rust1".to_string()),
                LineDiff::Equal("Status:: planned".to_string()),
            ]
        );
    }

    fn parsed(source: &str) -> ParsedSpec {
        ParsedSpec {
            source: source.to_string(),
            model: parse_spec(source),
        }
    }
}

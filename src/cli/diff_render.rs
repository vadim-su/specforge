use specforge::{
    diff::{
        LineDiff, ModelDiff, diff_lines, display_item_key, items_by_key, section_label,
        section_len, section_lines, source_lines, visible_diff_lines,
    },
    spec::{ParsedSpec, SpecItem, SpecKind},
};

use crate::cli::color::Colors;

pub fn print_diff(diff: &ModelDiff, colors: &Colors) {
    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        println!("No semantic changes.");
        return;
    }

    if !diff.added.is_empty() {
        println!("{}", colors.bold("Added:"));
        for item in &diff.added {
            let line = format!(
                "  + {} {:?}: {} (line {})",
                display_item_key(item),
                item.kind,
                item.title,
                item.source_range.start_line
            );
            println!("{}", colors.green(line));
        }
    }

    if !diff.removed.is_empty() {
        println!("{}", colors.bold("Removed:"));
        for item in &diff.removed {
            let line = format!(
                "  - {} {:?}: {}",
                display_item_key(item),
                item.kind,
                item.title
            );
            println!("{}", colors.red(line));
        }
    }

    if !diff.changed.is_empty() {
        println!("{}", colors.bold("Changed:"));
        for change in &diff.changed {
            if change.kind == SpecKind::Project {
                println!(
                    "  * project: {} (line {}) [{}]",
                    change.title,
                    change.line,
                    change.fields.join(", ")
                );
            } else {
                println!(
                    "  * {} {:?}: {} (line {}) [{}]",
                    change.id,
                    change.kind,
                    change.title,
                    change.line,
                    change.fields.join(", ")
                );
            }
        }
    }
}

pub fn print_text_diff(
    accepted: &ParsedSpec,
    current: &ParsedSpec,
    diff: &ModelDiff,
    colors: &Colors,
) {
    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        return;
    }

    let accepted_items = items_by_key(&accepted.model);
    let current_items = items_by_key(&current.model);
    let accepted_lines = source_lines(&accepted.source);
    let current_lines = source_lines(&current.source);

    println!();
    println!("{}", colors.bold("Text diff:"));

    for item in &diff.removed {
        print_removed_section(&accepted_lines, item, colors);
    }

    for item in &diff.added {
        print_added_section(&current_lines, item, colors);
    }

    for change in &diff.changed {
        let Some(before) = accepted_items.get(&change.id) else {
            continue;
        };
        let Some(after) = current_items.get(&change.id) else {
            continue;
        };

        print_changed_section(&accepted_lines, before, &current_lines, after, colors);
    }
}

fn print_removed_section(lines: &[&str], item: &SpecItem, colors: &Colors) {
    println!("{}", colors.bold(format!("--- {}", section_label(item))));
    println!("{}", colors.bold("+++ <removed>"));
    println!(
        "{}",
        colors.cyan(format!(
            "@@ -{},{} +0,0 @@",
            item.source_range.start_line,
            section_len(item)
        ))
    );

    for (idx, line) in section_lines(lines, item).iter().enumerate() {
        let line = format!(
            "- {:>5} {:>5} | {}",
            item.source_range.start_line + idx,
            "",
            line
        );
        println!("{}", colors.red(line));
    }
}

fn print_added_section(lines: &[&str], item: &SpecItem, colors: &Colors) {
    println!("{}", colors.bold("--- <added>"));
    println!("{}", colors.bold(format!("+++ {}", section_label(item))));
    println!(
        "{}",
        colors.cyan(format!(
            "@@ -0,0 +{},{} @@",
            item.source_range.start_line,
            section_len(item)
        ))
    );

    for (idx, line) in section_lines(lines, item).iter().enumerate() {
        let line = format!(
            "+ {:>5} {:>5} | {}",
            "",
            item.source_range.start_line + idx,
            line
        );
        println!("{}", colors.green(line));
    }
}

fn print_changed_section(
    accepted_lines: &[&str],
    before: &SpecItem,
    current_lines: &[&str],
    after: &SpecItem,
    colors: &Colors,
) {
    let old_lines = section_lines(accepted_lines, before);
    let new_lines = section_lines(current_lines, after);
    let line_diff = diff_lines(&old_lines, &new_lines);

    println!("{}", colors.bold(format!("--- {}", section_label(before))));
    println!("{}", colors.bold(format!("+++ {}", section_label(after))));
    println!(
        "{}",
        colors.cyan(format!(
            "@@ -{},{} +{},{} @@",
            before.source_range.start_line,
            section_len(before),
            after.source_range.start_line,
            section_len(after)
        ))
    );

    print_line_diff(
        &line_diff,
        before.source_range.start_line,
        after.source_range.start_line,
        3,
        colors,
    );
}

fn print_line_diff(
    diff: &[LineDiff],
    old_start_line: usize,
    new_start_line: usize,
    context: usize,
    colors: &Colors,
) {
    let visible = visible_diff_lines(diff, context);
    let mut old_line = old_start_line;
    let mut new_line = new_start_line;
    let mut skipped = false;

    for (idx, line) in diff.iter().enumerate() {
        let show = visible.contains(&idx);

        if show {
            if skipped {
                println!("{}", colors.dim(format!("  {:>5} {:>5} | ...", "", "")));
                skipped = false;
            }

            match line {
                LineDiff::Equal(text) => {
                    println!("  {:>5} {:>5} | {}", old_line, new_line, text);
                    old_line += 1;
                    new_line += 1;
                }
                LineDiff::Removed(text) => {
                    let line = format!("- {:>5} {:>5} | {}", old_line, "", text);
                    println!("{}", colors.red(line));
                    old_line += 1;
                }
                LineDiff::Added(text) => {
                    let line = format!("+ {:>5} {:>5} | {}", "", new_line, text);
                    println!("{}", colors.green(line));
                    new_line += 1;
                }
            }
        } else {
            skipped = true;
            match line {
                LineDiff::Equal(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                LineDiff::Removed(_) => {
                    old_line += 1;
                }
                LineDiff::Added(_) => {
                    new_line += 1;
                }
            }
        }
    }
}

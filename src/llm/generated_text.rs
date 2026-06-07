pub(crate) fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return ensure_trailing_newline(trimmed);
    };
    let Some(end) = rest.rfind("```") else {
        return ensure_trailing_newline(trimmed);
    };

    let inner = &rest[..end];
    let inner = inner
        .strip_prefix("asciidoc\n")
        .or_else(|| inner.strip_prefix("adoc\n"))
        .or_else(|| inner.strip_prefix("AsciiDoc\n"))
        .unwrap_or(inner)
        .trim();

    ensure_trailing_newline(inner)
}

fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

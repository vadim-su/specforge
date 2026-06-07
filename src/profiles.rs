#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TechProfile {
    pub id: &'static str,
    pub display_name: &'static str,
    pub applies_to: &'static [&'static str],
    pub conventions: &'static [&'static str],
    pub spec_focus: &'static [&'static str],
    pub validation_hints: &'static [&'static str],
}

impl TechProfile {
    fn matches_file(&self, file: &str) -> bool {
        self.applies_to
            .iter()
            .any(|pattern| file == *pattern || file.ends_with(pattern))
    }
}

pub fn builtin_profiles() -> &'static [TechProfile] {
    &[
        TechProfile {
            id: "rust",
            display_name: "Rust",
            applies_to: &["Cargo.toml", ".rs"],
            conventions: &[
                "Prefer explicit error behavior and Result boundaries.",
                "Mention CLI commands, flags, stdin/stdout, and filesystem side effects when relevant.",
                "Treat tests, formatting, and compiler checks as first-class acceptance evidence.",
            ],
            spec_focus: &[
                "Command behavior",
                "Error handling",
                "State and file formats",
                "Test/check commands",
            ],
            validation_hints: &[
                "If the spec implies a command, ask for inputs, outputs, exit behavior, and failure cases.",
                "If state is persisted, ask for file paths, schema shape, and migration/compatibility expectations.",
            ],
        },
        TechProfile {
            id: "typescript",
            display_name: "TypeScript",
            applies_to: &["package.json", "tsconfig.json", ".ts", ".tsx"],
            conventions: &[
                "Prefer typed contracts for API payloads, component props, and shared data models.",
                "Call out runtime validation when external input crosses type boundaries.",
                "Keep package scripts and generated artifacts explicit when they affect workflows.",
            ],
            spec_focus: &[
                "Typed contracts",
                "Build/test scripts",
                "Runtime validation",
                "Module boundaries",
            ],
            validation_hints: &[
                "Ask whether data shapes are compile-time only or also validated at runtime.",
                "Ask which package scripts prove the behavior when the project context shows scripts.",
            ],
        },
        TechProfile {
            id: "react",
            display_name: "React",
            applies_to: &[
                ".tsx",
                ".jsx",
                "vite.config.ts",
                "next.config.js",
                "next.config.ts",
            ],
            conventions: &[
                "Describe UI states explicitly: loading, error, empty, success, disabled, and pending.",
                "Capture component inputs, user events, navigation, and accessibility expectations.",
                "Prefer concrete interaction flows over broad screen descriptions.",
            ],
            spec_focus: &[
                "UI states",
                "User events",
                "Component props",
                "Accessibility",
            ],
            validation_hints: &[
                "Ask for missing screen states before implementation details.",
                "Ask how user actions should recover from errors or pending operations.",
            ],
        },
        TechProfile {
            id: "python",
            display_name: "Python",
            applies_to: &["pyproject.toml", "requirements.txt", ".py"],
            conventions: &[
                "Make command/module entrypoints, dependency boundaries, and error semantics explicit.",
                "Prefer concrete test runner expectations when project checks are implied.",
                "Call out configuration sources such as env vars, files, and CLI flags.",
            ],
            spec_focus: &[
                "Entrypoints",
                "Configuration",
                "Error behavior",
                "Test strategy",
            ],
            validation_hints: &[
                "Ask which inputs come from CLI args, environment, files, or API calls.",
                "Ask what exceptions or validation failures should look like to users.",
            ],
        },
        TechProfile {
            id: "fastapi",
            display_name: "FastAPI",
            applies_to: &["fastapi", "main.py", "api.py", "routers/"],
            conventions: &[
                "Describe endpoints with method, path, request body, response body, and status codes.",
                "Capture authentication, dependencies, validation, and OpenAPI-visible behavior.",
                "Keep API errors and business errors distinct.",
            ],
            spec_focus: &[
                "Endpoint contracts",
                "Pydantic schemas",
                "Authentication/dependencies",
                "Status codes",
            ],
            validation_hints: &[
                "Ask for response status codes and error bodies when endpoint behavior is implied.",
                "Ask whether generated OpenAPI shape is part of the acceptance contract.",
            ],
        },
        TechProfile {
            id: "postgres",
            display_name: "Postgres",
            applies_to: &["migrations/", "schema.sql", ".sql"],
            conventions: &[
                "Make tables, columns, constraints, indexes, and relationships explicit.",
                "Treat destructive migrations and backfills as decisions with safety constraints.",
                "Ask for query patterns when indexes or uniqueness are implied.",
            ],
            spec_focus: &["Data model", "Constraints", "Indexes", "Migration safety"],
            validation_hints: &[
                "Ask for uniqueness and foreign-key behavior when entities reference each other.",
                "Ask whether migration rollback or data backfill behavior is required.",
            ],
        },
    ]
}

pub fn detect_profiles(files: &[String]) -> Vec<TechProfile> {
    builtin_profiles()
        .iter()
        .filter(|profile| files.iter().any(|file| profile.matches_file(file)))
        .cloned()
        .collect()
}

pub fn render_profiles_prompt(profiles: &[TechProfile]) -> String {
    if profiles.is_empty() {
        return "No technology profiles were detected from project files.".to_string();
    }

    profiles
        .iter()
        .map(|profile| {
            format!(
                "<profile id=\"{}\" name=\"{}\">\n<conventions>\n{}\n</conventions>\n<spec-focus>\n{}\n</spec-focus>\n<validation-hints>\n{}\n</validation-hints>\n</profile>",
                profile.id,
                profile.display_name,
                render_items(profile.conventions),
                render_items(profile.spec_focus),
                render_items(profile.validation_hints)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_items(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_profiles_from_project_files() {
        let files = vec![
            "Cargo.toml".to_string(),
            "src/main.rs".to_string(),
            "README.md".to_string(),
        ];

        let ids = detect_profiles(&files)
            .into_iter()
            .map(|profile| profile.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["rust"]);
    }

    #[test]
    fn detects_multiple_profiles() {
        let files = vec![
            "package.json".to_string(),
            "src/App.tsx".to_string(),
            "migrations/001_init.sql".to_string(),
        ];

        let ids = detect_profiles(&files)
            .into_iter()
            .map(|profile| profile.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["typescript", "react", "postgres"]);
    }
}

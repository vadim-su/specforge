<p align="center">
  <img src="assets/specforge-logo.png" alt="SpecForge logo" width="720">
</p>

# SpecForge

SpecForge is a spec-driven project state tool for turning product ideas into a
canonical project spec, validating that spec, diffing it against stored state,
and driving implementation work from the accepted changes.

## Features

- Initialize `spec.adoc` from prose with an LLM or a deterministic template.
- Validate a restricted AsciiDoc spec model before accepting changes.
- Print normalized spec models as JSON for inspection and automation.
- Diff the current spec against `.specforge/state/current.adoc`.
- Normalize spec tags, accept the new state, and optionally run the development
  agent for the detected changes.
- Ask for targeted product and engineering questions that can expand a spec.
- Detect technology profiles from project files and use stack-specific guidance
  while expanding specs.
- Declare external context providers, including future MCP adapters, in project
  config.
- Apply ad-hoc code fixes with the code change agent.
- Run configured project checks and ask the agent to add or improve tests for
  plain-text targets, files, or spec items.
- Use OpenAI, Anthropic, Ollama, or ChatGPT providers.

## Installation

```sh
cargo install --path .
```

Or run the CLI directly from the repository:

```sh
cargo run -- --help
```

## Shell Completions

SpecForge can generate completion scripts for shells supported by `clap`:

```sh
specforge completions bash
specforge completions zsh
specforge completions fish
specforge completions powershell
specforge completions elvish
```

Install examples:

```sh
mkdir -p ~/.local/share/bash-completion/completions
specforge completions bash > ~/.local/share/bash-completion/completions/specforge

mkdir -p ~/.zfunc
specforge completions zsh > ~/.zfunc/_specforge

mkdir -p ~/.config/fish/completions
specforge completions fish > ~/.config/fish/completions/specforge.fish
```

For zsh, ensure `~/.zfunc` is in `fpath` and `compinit` is loaded from
`.zshrc`.

## Quick Start

Create a starter spec without an LLM:

```sh
specforge init --template --output spec.adoc
```

Validate the spec:

```sh
specforge check spec.adoc
```

Inspect the parsed model:

```sh
specforge model spec.adoc
```

Accept the current spec as project state:

```sh
specforge accept spec.adoc
```

Show the next diff:

```sh
specforge diff spec.adoc
```

Synchronize spec changes and skip agent execution:

```sh
specforge sync spec.adoc --skip-agent
```

Run configured project checks:

```sh
specforge test run
```

Ask the agent to improve test coverage for a plain-text target, a file, or a
spec item:

```sh
specforge test cover "sync rejects invalid specs"
specforge test cover --file src/sync.rs
specforge test cover --item feature-sync
```

Apply an ad-hoc code change from text and optional screenshots:

```sh
specforge fix "Add input validation to the sync flow"
specforge fix --image screenshot.png "Fix the broken UI state shown here"
wl-paste --type image/png | specforge fix --image - "Fix the UI state in this screenshot"
pngpaste - | specforge fix --image - "Fix the UI state in this screenshot"
```

Answer targeted questions in a terminal questionnaire and get conclusions for
expanding a spec:

```sh
specforge assist expand spec.adoc
```

## LLM Providers

Commands that call an LLM accept `--provider` and `--model`:

```sh
specforge init idea.md --provider openai --model gpt-5-nano
specforge sync spec.adoc --provider anthropic
specforge fix "Add input validation to the sync flow" --provider ollama
```

Provider setup depends on the selected backend:

- `openai`: set `OPENAI_API_KEY`.
- `anthropic`: set `ANTHROPIC_API_KEY`.
- `ollama`: set `OLLAMA_API_BASE_URL` if not using localhost.
- `chatgpt`: set `CHATGPT_ACCESS_TOKEN` or complete OAuth.

## Project Checks

SpecForge validates agent-applied patches with commands from
`.specforge/config.yaml`. During LLM-backed `init`, SpecForge asks the model to
infer this config from the project idea and selected preferences:

```yaml
checks:
  - command: ["cargo", "fmt", "--check"]
    timeout_seconds: 30
  - command: ["cargo", "test", "--color", "never"]
    timeout_seconds: 120
```

Configure as many checks as the project needs; SpecForge runs them in order.
When no checks are configured, project checks are skipped. The agent also
receives the active check plan before generating patches.

## Agent File Access

By default, the agent file tools can inspect repository files except `.git`,
`target`, `.specforge`, and SpecForge-owned spec files. To restrict which files
can be listed or read by the agent, configure `file_access.allowed` in
`.specforge/config.yaml`:

```yaml
file_access:
  allowed:
    - Cargo.toml
    - src/
    - examples/**
```

An empty or omitted `allowed` list keeps the default unrestricted repository
access. File entries match exact files. Directory entries ending in `/` or
`/**` match files under that directory. The active file access policy is stored
with each agent task so resumed tasks keep using the same policy.

## Technology Profiles and Context Providers

`specforge assist expand` builds a normalized context bundle from the project
before asking targeted spec questions. The bundle currently includes:

- a filesystem provider that lists project files and reads selected source/doc
  snippets;
- detected technology profiles such as Rust, TypeScript, React, Python,
  FastAPI, and Postgres;
- declared MCP integration slots from `.specforge/config.yaml`.

Profiles are data-driven stack hints. They guide questions toward details the
stack makes important: CLI behavior for Rust, UI states for React, endpoint
contracts for FastAPI, migration safety for Postgres, and similar concerns.

MCP servers are declared under `integrations.mcp`:

```yaml
integrations:
  mcp:
    context7:
      command: "npx"
      args: ["-y", "@upstash/context7-mcp"]
      env_vars: ["LOCAL_TOKEN"]
      env:
        MY_ENV_VAR: "MY_ENV_VALUE"
```

`env_vars` lists additional variable names to inherit from the process
environment. SpecForge also preserves a small baseline environment such as
`PATH` and home-directory variables so command-based servers like `npx` can
start. `env` contains inline overrides for that MCP server. SpecForge may
mention env key names in LLM context, but it does not include inline env values
in prompts.

When an agent-backed command runs, SpecForge starts enabled MCP servers over
stdio, registers their exposed tools in Rig's shared tool server, and shuts them
down when the command exits. Local SpecForge agent tools and MCP tools share the
same Rig tool surface, while local filesystem and patch tools still execute
through SpecForge's guarded agent loop.

## CLI Commands

```text
specforge init [INPUT] [--output spec.adoc] [--template] [--force]
specforge check [SPEC]
specforge model [SPEC]
specforge diff [SPEC]
specforge sync [SPEC] [--yes] [--skip-agent]
specforge test run
specforge test cover [TARGET...] [--file PATH] [--item ID_OR_TITLE] [--spec spec.adoc]
specforge assist expand [SPEC]
specforge fix [--image PATH] [REQUEST...]
specforge completions <SHELL>
specforge accept [SPEC]
```

Use `--project-root DIR` with any command to run against another project root.

## Project State

SpecForge writes accepted state under `.specforge/state/`. Generated or resumed
agent tasks live under `.specforge/tasks/`. The agent file tools intentionally
exclude `.git`, `target`, `.specforge`, and SpecForge-owned spec files.

## License

SpecForge is licensed under the [MIT License](LICENSE).

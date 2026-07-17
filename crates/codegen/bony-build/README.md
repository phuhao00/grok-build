# Bony Build

Native desktop client for **Bony Build**. Built with **eframe/egui** and talks to the
agent over **ACP stdio** (`grok agent stdio`).

## Prerequisites

1. A `grok` binary on `PATH` (e.g. `npm i -g @xai-official/grok`)
2. Auth — pick one:
   - `grok login` / `XAI_API_KEY` (xAI), **or**
   - any provider via `~/.grok/config.toml` `[model.*]` with `api_key` / `env_key`
3. Rust toolchain (see repo `rust-toolchain.toml`)

### Custom / arbitrary LLM providers

Desktop does not hardcode a single vendor. Model routing comes from the agent +
`%USERPROFILE%\.grok\config.toml`. Example (OpenAI-compatible):

```toml
[models]
default = "gpt-4o"

[model.gpt-4o]
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
name = "GPT-4o"
env_key = "OPENAI_API_KEY"
context_window = 128000
```

Also supported: `api_backend = "messages"` (Anthropic), `"responses"`, local
Ollama (`http://localhost:11434/v1`), Together, company gateways
(`GROK_MODELS_BASE_URL`), etc. See
`crates/codegen/xai-grok-pager/docs/user-guide/11-custom-models.md`.

After editing config, set the matching env var and **restart** the desktop app.
`grok models` should list your custom id.

On Windows, if `cargo build` fails with **os error 4551** (application control
policy blocked), disable **Smart App Control** or build from a trusted terminal
outside restricted environments, then retry.

## Run

```powershell
cd c:\Users\HHaou\grok-build
$env:CARGO_TARGET_DIR = "$PWD\target"
cargo run -p bony-build
# or
powershell -ExecutionPolicy Bypass -File .\scripts\run-desktop.ps1
```

Options:

```text
--cwd <path>           Session working directory (default: cwd)
--grok-bin <path>      Path to grok executable
--ask-permissions      Require manual tool approval (default: auto-approve)
```

## What you get

- Chat window with streaming assistant text
- Model picker (session switch + persist default)
- Inline tool cards and permission Approve / Deny
- Cancel in-flight turn
- CJK UI fonts on Windows

## Architecture

```text
Bony Build (egui)
    │  ACP JSON-RPC over stdio
    ▼
grok agent stdio  →  MvpAgent / SessionActor
```

This crate does **not** embed the full agent runtime; it drives the installed
`grok` binary as a subprocess.

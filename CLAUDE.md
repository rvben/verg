# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Build & Test Commands

```bash
make check          # lint + test (CI runs this)
make build          # cargo build
make release        # cargo build --release
make test           # cargo test
make lint           # cargo fmt --check + cargo clippy -D warnings
make fmt            # auto-format
make install        # build release and copy to ~/.local/bin/
make e2e            # run end-to-end tests (requires Docker)
```

## Architecture

verg is a stateless desired-state infrastructure convergence engine. Two binaries from one crate:

- `verg` — control CLI (runs on your machine)
- `verg-agent` — pushed to targets over SSH, executes resources locally

### Source layout

**`src/bin/verg.rs`** — CLI entry point. Clap-derived commands (apply, diff, check, schema, init, completions). Global flags: `--json`, `--path`, `--parallel`, `--ssh-config`.

**`src/bin/verg_agent.rs`** — Agent binary. Reads TOML bundle from stdin, executes resources in DAG order, outputs JSON summary to stdout.

**`src/inventory/`** — Host inventory system. `static_hosts.rs` parses `hosts.toml`, `groups.rs` loads group variables, `selector.rs` parses target selectors (`web`, `prod:!db`, `host1,host2`). `mod.rs` merges everything and provides `filter()`.

**`src/state/`** — State file parsing. `mod.rs` loads TOML state files from `state/` directory. `vars.rs` handles `{{ var }}` interpolation.

**`src/resources/`** — Resource executors. `mod.rs` defines types (`ResourceResult`, `RunSummary`, `ResolvedResource`) and `execute_resource()` dispatcher. `dag.rs` resolves execution order via topological sort. Individual resources: `pkg.rs`, `file.rs`, `service.rs`, `cmd.rs`, `user.rs`.

**`src/bundle.rs`** — Builds host-specific task payloads from state files with variable interpolation.

**`src/transport/ssh.rs`** — SSH transport. Pushes agent binary, pipes bundle to stdin, parses JSON results.

**`src/engine.rs`** — Orchestrates parallel execution across targets via tokio + semaphore.

**`src/commands/`** — CLI command implementations (apply, diff, check, init).

**`src/output.rs`** — Output config (JSON auto-detect, terminal color detection).

**`src/schema.rs`** — Resource type schema for agent introspection.

**`src/changelog.rs`** — Writes structured JSON logs per apply run.

### Key patterns

- **Config convention**: `verg/` directory in CWD, overridable with `--path` or `VERG_PATH`
- **Output**: Data to stdout, messages to stderr. JSON auto-enabled when piped.
- **Exit codes**: 0 (success+changes), 1 (nothing changed), 2 (partial failure), 3 (total failure), 4 (connection), 5 (config), 6 (target not found), 7 (internal)
- **Resource DAG**: Resources declare `after = ["pkg.nginx"]` dependencies. No `after` = parallel.
- **Stateless**: Every run checks reality. No state files.

### Adding a new resource type

1. Create `src/resources/<type>.rs` with `pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error>`
2. Add `pub mod <type>;` to `src/resources/mod.rs`
3. Add match arm in `execute_resource()` in `src/resources/mod.rs`
4. Add schema entry in `src/schema.rs`

### E2E testing

`make e2e` spins up a Docker container with sshd, applies state, and verifies convergence. Requires Docker.

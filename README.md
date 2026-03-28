# verg

Desired-state infrastructure convergence engine. A fast, stateless alternative to Ansible, built in Rust.

## Features

- **Fast** — pushes a single static binary to targets over SSH, executes locally. No Python, no per-task SSH round-trips.
- **Predictable** — stateless convergence. Every run checks reality and converges. No state files.
- **Simple** — TOML declarations. No YAML, no Jinja2, no variable precedence maze.
- **Agent-friendly** — `--json` output, `schema` command, structured exit codes. Built for both humans and AI agents.

## Quick Start

```sh
# Install
cargo install verg

# Initialize a project
verg init

# Edit hosts and state
vim verg/hosts.toml
vim verg/state/base.toml

# Preview changes
verg diff --targets all

# Apply
verg apply --targets all
```

## How It Works

1. Declare desired state in TOML files under `verg/state/`
2. Define hosts in `verg/hosts.toml` with group assignments
3. Run `verg apply` — the tool pushes a binary to each target over SSH, which checks current state and converges

## Resource Types

| Type | Description |
|------|-------------|
| `pkg` | System packages (apt, dnf, pacman — auto-detected) |
| `file` | Files and directories (content, permissions, ownership) |
| `service` | Systemd services (running/stopped, enabled/disabled) |
| `cmd` | Run a command (requires idempotency guard) |
| `user` | System users (create/remove) |

## Example

```toml
# verg/hosts.toml
[hosts.web1]
address = "192.168.1.10"
user = "root"
groups = ["web"]

# verg/state/web.toml
targets = ["web"]

[resource.pkg.nginx]
name = "nginx"
state = "present"

[resource.file.nginx-conf]
path = "/etc/nginx/nginx.conf"
content = "server { listen {{ http_port }}; }"
after = ["pkg.nginx"]

[resource.service.nginx]
name = "nginx"
state = "running"
enabled = true
after = ["file.nginx-conf"]
```

## Commands

| Command | Description |
|---------|-------------|
| `verg apply -t <targets>` | Converge targets to desired state |
| `verg diff -t <targets>` | Show what would change (dry-run) |
| `verg check -t <targets>` | Verify targets match desired state |
| `verg schema` | Print resource type schemas as JSON |
| `verg init` | Scaffold a new project |
| `verg completions <shell>` | Generate shell completions |

## License

MIT

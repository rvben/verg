[![CI](https://github.com/rvben/verg/actions/workflows/ci.yml/badge.svg)](https://github.com/rvben/verg/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/verg.svg)](https://crates.io/crates/verg)
[![PyPI](https://img.shields.io/pypi/v/verg.svg)](https://pypi.org/project/verg/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![codecov](https://codecov.io/gh/rvben/verg/graph/badge.svg)](https://codecov.io/gh/rvben/verg)

# verg

Desired-state infrastructure convergence engine. A fast, stateless alternative to Ansible, built in Rust.

> **Pre-1.0:** verg is under active development. Expect breaking changes between minor versions.

## Features

- **Fast** - pushes a single static binary to each target over SSH; the agent executes locally, so there are no per-task SSH round-trips and no Python dependency on the target.
- **Stateless** - every run reads actual system state and converges to desired state. No state files are kept on the control host between runs.
- **Simple** - declare state in TOML files with Jinja2 templating for dynamic values. Strict config validation catches typos before any SSH connection is opened.
- **Agent-friendly** - `--json` output, `verg schema` for machine-readable resource schemas, and structured exit codes make verg easy to drive from scripts and AI agents.
- **Secure by default** - agent binary is checksum-verified before and after transfer; strict SSH host key checking; config validated locally before touching any target.

## Install

```sh
# Cargo (crates.io)
cargo install verg

# PyPI
pip install verg

# Homebrew (tap)
brew install rvben/tap/verg
```

## Quick Start

```sh
# Scaffold a new project in ./verg/
verg init

# Edit your inventory and desired state
$EDITOR verg/hosts.toml
$EDITOR verg/state/base.toml

# Preview what would change (dry-run, no modifications)
verg diff --targets all

# Apply to all hosts
verg apply --targets all
```

## How It Works

`verg` (control CLI, runs on your machine) reads TOML state files and your host inventory, builds a host-specific bundle for each target, then pushes a compiled `verg-agent` binary over SSH. The agent reads the bundle from stdin, checks actual system state, converges each resource to desired state in dependency order, and returns a JSON summary to stdout. The control CLI aggregates results and presents them.

The agent is a static binary with no runtime dependencies. It is checksum-verified before deployment and runs entirely on the target - no per-resource SSH calls from the control host are needed once the agent is running.

## Concepts

### Project layout

A verg project lives in a directory named `verg/` by default (override with `--path` or the `VERG_PATH` environment variable). Inside it:

```
verg/
  hosts.toml       # host inventory
  groups/          # one .toml per group, each with a [vars] table
  state/           # desired-state declarations; loaded in lexicographic order
  files/           # static files referenced by file resources
  templates/       # Jinja2 template files
  .verg/logs/      # structured apply logs (written automatically)
```

`verg init` scaffolds this layout with a starter `hosts.toml` and `state/base.toml`.

### Inventory and selectors

Each host in `hosts.toml` has these fields:

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `address` | yes | - | IP address or hostname |
| `user` | no | `"root"` | SSH user |
| `port` | no | `22` | SSH port |
| `groups` | no | `[]` | Group memberships |
| `[hosts.NAME.vars]` | no | - | Host-specific variables |

The `--targets` flag accepts a selector expression:

| Syntax | Meaning |
|--------|---------|
| `all` | Every host |
| `web` | Hosts named `web` or in group `web` |
| `a,b` | Union of selectors `a` and `b` |
| `a:b` | Intersection (hosts in both `a` and `b`) |
| `!x` | Exclude `x` |
| `prod:!db` | In group `prod` but not group `db` |

An unknown selector name is an error (exit code 6). Parentheses are not supported.

### State files

State files live in `state/` and are loaded in lexicographic order. Each file is a TOML document with this shape:

```toml
targets = ["web"]          # optional: limit to hosts in group/name "web"
                           # omit to apply to all hosts

[resource.<type>.<name>]   # one table per resource
property = "value"
```

A state file with no `targets` key applies to every host. A state file with `targets = ["web"]` applies only when the target host belongs to group `web` or has the name `web`. Only `targets` and `resource` are valid top-level keys; unrecognized keys are rejected by default.

See [docs/resources.md](docs/resources.md) for the full resource reference.

### Variables and facts

Variables are interpolated using Jinja2 syntax (`{{ var }}`). Template sources:

| Source | Precedence |
|--------|-----------|
| Host vars (`[hosts.NAME.vars]`) | Highest |
| Group vars (`groups/<g>.toml` `[vars]`) | Below host vars |
| Facts gathered at runtime | Available as `fact.arch`, `fact.os`, etc. |
| Group membership | Available as `group.<name>` |
| Inventory context (`hosts.*`, `groups.*`) | Lowest |

Available facts: `fact.arch`, `fact.hostname`, `fact.os`, `fact.os_release`, `fact.os_version`.

String values starting with `$env.` (e.g. `password = "$env.MY_SECRET"`) are resolved from the environment at build time.

Inline `content` in a `file` resource is always interpolated. To interpolate a file loaded via `source`, set `template = true`.

`cmd` resources can `register` their stdout for use in downstream resources as `{{ register.NAME }}`. See [docs/resources.md](docs/resources.md) for details.

### Ordering and handlers

Resources declare dependencies with `after = ["type.name", ...]`. verg sorts resources into execution layers using Kahn's topological sort; within a layer, resources run in FQN order. A resource whose dependency fails is skipped with "dependency failed".

Resources with `handler = true` are skipped unless another resource lists their FQN in `notify`. Handlers run after all normal resources; shorthand notify actions (e.g. `reload:nginx`, `daemon-reload`) run after handlers. Each shorthand runs at most once per host per run.

`when` expressions conditionally skip resources based on facts and group membership (e.g. `when = "fact.os == 'ubuntu'"`).

See [docs/resources.md](docs/resources.md) for the full common attributes reference.

## Resource Types

| Type | Purpose |
|------|---------|
| [`pkg`](docs/resources.md#pkg) | Install or remove system packages (apt-get, dnf, pacman - auto-detected) |
| [`file`](docs/resources.md#file) | Manage a file's content, permissions, and ownership |
| [`directory`](docs/resources.md#directory) | Ensure a directory exists with the right permissions and ownership |
| [`service`](docs/resources.md#service) | Manage a systemd service's running state and boot enablement |
| [`cmd`](docs/resources.md#cmd) | Run a shell command with an idempotency guard |
| [`user`](docs/resources.md#user) | Create or remove a system user |
| [`cron`](docs/resources.md#cron) | Manage a cron job file in `/etc/cron.d/` |
| [`sysctl`](docs/resources.md#sysctl) | Set a Linux kernel parameter |
| [`apt_repo`](docs/resources.md#apt_repo) | Add or remove an APT repository with its GPG key |
| [`docker_compose`](docs/resources.md#docker_compose) | Manage a Docker Compose stack |
| [`download`](docs/resources.md#download) | Download a file from a URL, optionally extracting an archive |

## Complete Example

A minimal nginx setup using pkg, file, and service resources with dependency ordering.

```toml
# verg/hosts.toml
[hosts.web1]
address = "192.0.2.10"
user = "root"
groups = ["web"]

[hosts.web1.vars]
http_port = "80"
server_name = "example.com"
```

```toml
# verg/state/nginx.toml
targets = ["web"]

[resource.pkg.nginx]
name = "nginx"
state = "present"

[resource.file.nginx-conf]
path = "/etc/nginx/sites-available/default"
content = """
server {
    listen {{ http_port }};
    server_name {{ server_name }};
    root /var/www/html;
}
"""
mode = "0644"
after = ["pkg.nginx"]
notify = ["reload:nginx"]

[resource.service.nginx]
name = "nginx"
state = "running"
enabled = true
after = ["file.nginx-conf"]
```

The FQN for each resource is `type.name` (e.g. `pkg.nginx`, `file.nginx-conf`, `service.nginx`). Every `after` reference in this example resolves to a resource defined in the same file.

## Commands

| Command | Key args | Description |
|---------|----------|-------------|
| `verg apply` | `--targets <TARGETS>` (required) | Converge targets to desired state |
| `verg diff` | `--targets <TARGETS>` (default: `all`), `--limit`, `--offset`, `--fields` | Show what would change without applying |
| `verg check` | `--targets <TARGETS>` (default: `all`) | Verify targets match desired state (exits 0 when drift found, 1 when all match) |
| `verg schema` | - | Print resource type schemas as JSON |
| `verg init` | `--force` | Scaffold a new project directory |
| `verg completions` | `<bash\|fish\|zsh\|powershell\|elvish>` | Generate shell completions |

`apply` has no default target. `--targets` is required to prevent accidental mass applies. Running `apply` in a non-interactive pipeline without `--yes` exits with code 5 (confirmation required).

## Global Flags

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output <auto\|text\|json>` | `auto` | Output format; `auto` selects JSON when stdout is not a TTY |
| `--json` | - | Force JSON output (hidden alias for `-o json`) |
| `-q, --quiet` | - | Suppress per-resource lines; print only the final summary |
| `-y, --yes` | - | Proceed when stdin is not a TTY (required for CI/pipelines) |
| `--path <PATH>` | `./verg` | Path to the verg project directory (`VERG_PATH`) |
| `--parallel <N>` | `10` | Maximum parallel SSH connections |
| `--ssh-config <PATH>` | - | Path to SSH config file (`VERG_SSH_CONFIG`) |
| `--agent-dir <PATH>` | - | Directory containing verg-agent binaries per architecture (`VERG_AGENT_DIR`) |
| `--host-key-checking <yes\|accept-new\|no>` | `yes` | SSH host key checking policy |
| `--ssh-known-hosts <PATH>` | - | Path to a known_hosts file |
| `--skip-agent-checksum` | - | Skip agent binary checksum verification (air-gapped or local builds) |
| `--lax-config` | - | Downgrade config validation errors (unknown keys/types) to warnings |
| `--timeout <SECONDS>` | `600` | Per-host timeout in seconds |

## Exit Codes

| Code | Name | Meaning |
|------|------|---------|
| `0` | SUCCESS | Resources changed or drift detected |
| `1` | NOTHING_CHANGED | All resources already in desired state |
| `2` | PARTIAL_FAILURE | Some resources failed; others succeeded |
| `3` | TOTAL_FAILURE | All resources on all targets failed |
| `4` | CONNECTION_ERROR | All failures were SSH connection errors |
| `5` | INVALID_CONFIG | Config is invalid, or `apply` requires `--yes` (ConfirmationRequired) |
| `6` | TARGET_NOT_FOUND | No hosts matched the given target selector |
| `7` | INTERNAL_ERROR | Unexpected internal error (I/O or other) |
| `8` | CONFLICT | State conflict that cannot be automatically resolved |
| `130` | INTERRUPTED | Process received SIGINT (Ctrl-C) |

`check` exits 0 when drift is found and 1 when everything already matches. `diff` exits 0 on any successful run (even with no changes); only connection or partial failures yield 4 or 2.

## Security

verg pushes a compiled agent binary to target hosts and runs it as the configured SSH user (root by default). The agent binary is SHA-256 verified locally before transfer and remotely after transfer using `sha256sum -c`. SSH host key checking is strict by default (`StrictHostKeyChecking=yes`). All state files are validated locally before any SSH connection is opened. Resources marked `sensitive = true` have their payload redacted from output and the apply changelog.

See [SECURITY.md](SECURITY.md) for the full threat model, supply chain details, secret handling, and vulnerability reporting.

## License

MIT

# Continuous Enforcement

Continuous enforcement lets each host converge itself on a schedule without any central server or control-host connection. The control host publishes bundles offline; agents pull and re-converge independently.

## The Pull Model

```
Control host                     Target hosts
-----------                      ------------
verg publish                     verg-agent serve
  reads state files                 pulls <host>.toml on a schedule
  builds per-host bundles    --->   converges to desired state
  writes <dest>/<host>.toml         writes a redacted run report
  (no SSH, no network)              (no connection back to control)
```

No central server is involved. The control host runs `verg publish` to produce one bundle file per host, then makes those files available to the hosts (HTTP file server, object storage, rsync to each host, or any other mechanism). Each agent fetches its own bundle and runs convergence cycles independently.

## Publishing Bundles: `verg publish`

```sh
verg publish --targets <SELECTOR> --dest <DIR>
```

`verg publish` builds host-specific bundles offline (no SSH connections) and writes one `<host>.toml` file per matched host into `<DIR>`.

### What is included in each bundle

The bundle contains all resources that apply to the host, after variable interpolation and template rendering. It also includes any custom resource definitions from `verg/resources/`.

Group membership is injected as facts (`group.<name> = "true"`) so that `when` conditions and templates that check `{{ group.web }}` work correctly in the offline agent.

### The offline facts limitation

`verg publish` runs with no SSH connection and therefore cannot gather live system facts. The following facts are NOT available during publish:

- `fact.arch`
- `fact.hostname`
- `fact.os`
- `fact.os_release`
- `fact.os_version`

A template that references `{{ fact.arch }}` (or any other `fact.*` variable) will fail to render for that host because the variable is undefined. The failure is per-host and non-fatal: other hosts whose bundles do not reference that variable will still be published.

**Workaround:** Add the needed facts as host variables in `hosts.toml`:

```toml
[hosts.web1]
address = "192.0.2.10"
groups = ["web"]

[hosts.web1.vars]
fact.arch = "x86_64"
fact.os = "ubuntu"
```

Alternatively, use `verg apply` (live SSH push) for configurations that depend heavily on gathered facts.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | All matched hosts published successfully |
| `2` | Some hosts published, some failed (partial failure) |
| `5` | No hosts published: either every host failed to build, or project-level config validation failed before any bundle was written |

Project-level validation failures (unknown resource types, invalid config) cause an immediate non-zero return before any output is written. Per-host bundle failures (e.g. unresolved fact template variables) are logged to stderr and do not prevent the remaining hosts from publishing.

### Selector syntax

The `--targets` selector uses the same syntax as `verg apply`:

| Syntax | Meaning |
|--------|---------|
| `all` | Every host in the inventory |
| `web` | Hosts named `web` or in group `web` |
| `a,b` | Union of selectors `a` and `b` |
| `prod:!db` | In group `prod` but not group `db` |

## Running the Agent: `verg-agent serve`

```sh
verg-agent serve --source <PATH|URL> --interval <DURATION>
verg-agent serve --source <PATH|URL> --once
```

`verg-agent serve` is the pull-mode entry point of the agent. It fetches the bundle from `--source`, converges the host, writes a run report, then sleeps for `--interval` before repeating.

### Source types

`--source` accepts either a local filesystem path or an `http://` or `https://` URL:

```sh
# Local path
verg-agent serve --source /etc/verg/bundle.toml --interval 30m

# HTTPS URL (fetched with curl)
verg-agent serve --source https://bundles.example.com/web1.toml --interval 30m
```

HTTPS sources are fetched using `curl -fsSL`. A non-zero curl exit (404, connection failure, TLS error) is treated as a transient error in loop mode: it is logged to stderr and the daemon sleeps until the next interval. The daemon does not exit on fetch or parse failures.

### `--interval` and `--once`

`--interval` is required when `--once` is not set. A zero interval is rejected.

`--once` runs a single convergence cycle and exits with the following codes:

| Code | Meaning |
|------|---------|
| `0` | Changes were applied (at least one resource changed) |
| `1` | Nothing changed (host already in desired state) |
| `2` | Some resources failed, others succeeded |
| `3` | All resources failed |
| `5` | Fetch or parse error (could not load or read the bundle) |

In loop mode (with `--interval`), errors from individual cycles are logged but the daemon stays up and retries on the next interval.

### `--report-dir`

Default: `/var/lib/verg/runs`

After each convergence cycle, the agent writes a JSON report to `<report-dir>/<timestamp>-serve.json`. Reports are redacted: payload bodies (`from`, `to`, `output`) are stripped and long diffs are truncated to 200 characters. This matches the apply-log redaction policy and avoids persisting secrets on disk.

The report format:

```json
{
  "timestamp": "2026-06-21T10-30-00-000000000",
  "source": "https://bundles.example.com/web1.toml",
  "summary": {
    "host": "web1",
    "resources": [...],
    "summary": { "ok": 3, "changed": 1, "failed": 0, "skipped": 0 }
  }
}
```

## Duration Format

The `--interval` flag accepts durations in these formats:

| Format | Example | Meaning |
|--------|---------|---------|
| Bare integer | `45` | 45 seconds |
| Seconds | `30s` | 30 seconds |
| Minutes | `5m` | 5 minutes |
| Hours | `2h` | 2 hours |
| Days | `1d` | 1 day |

Decimals and negative values are rejected. A zero duration is rejected for `--interval` (it would busy-loop).

## Deployment Pattern

Each host fetches only its own bundle. A typical setup:

1. Run `verg publish --targets all --dest /tmp/bundles` on the control host.
2. Serve or copy the bundles so each host can reach its own file. For example:
   - Serve the directory over HTTPS and point each agent at `https://bundles.example.com/<hostname>.toml`.
   - Rsync each `<host>.toml` to the corresponding host and use a local path as `--source`.
3. On each host, run `verg-agent serve --source <its-bundle-url> --interval 30m` as a systemd service.

The control host has no long-running process. Publish runs are typically triggered by CI on each config change.

### One-time bootstrap

Install `verg-agent` on each host once (e.g. via `verg apply` or a provisioning tool), then the agent self-converges going forward. No additional connections from the control host are needed.

## Systemd Examples

### Option A: Long-lived service

The agent runs continuously and wakes on its own interval. Suitable for frequent polling (every few minutes).

```ini
# /etc/systemd/system/verg-agent.service
[Unit]
Description=verg infrastructure convergence agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/verg-agent serve \
  --source https://192.0.2.1/bundles/web1.toml \
  --interval 30m \
  --report-dir /var/lib/verg/runs
Restart=on-failure
RestartSec=60s

[Install]
WantedBy=multi-user.target
```

Enable and start:

```sh
systemctl daemon-reload
systemctl enable --now verg-agent.service
```

### Option B: Timer-driven one-shot

The system timer schedules convergence; the service exits after each run. Suitable for infrequent runs (hourly, daily) where systemd timer management is preferred over an in-process sleep loop.

```ini
# /etc/systemd/system/verg-agent-once.service
[Unit]
Description=verg infrastructure convergence (one-shot)
After=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/verg-agent serve \
  --source https://192.0.2.1/bundles/web1.toml \
  --once \
  --report-dir /var/lib/verg/runs
```

```ini
# /etc/systemd/system/verg-agent-once.timer
[Unit]
Description=Run verg convergence every 30 minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=30min
Persistent=true

[Install]
WantedBy=timers.target
```

Enable and start:

```sh
systemctl daemon-reload
systemctl enable --now verg-agent-once.timer
```

Check the timer:

```sh
systemctl list-timers verg-agent-once.timer
```

# Security

## Threat model and trust assumptions

verg pushes a compiled agent binary to target hosts over SSH and runs it as the configured SSH user (root by default). Because the agent executes with that user's privileges, the control host has full authority over the target. This is intentional: verg needs root-level access to converge system state (install packages, write files to system paths, manage services, etc.).

The following must be trusted for a secure deployment:

- **The control host.** All state files, templates, and the agent binary originate here. A compromised control host can push arbitrary changes to every managed target.
- **The SSH credentials.** Keys or certificates used to authenticate to targets. Compromise of these allows impersonation of the control host.
- **The agent binary supply chain.** The binary pushed to targets is downloaded from GitHub releases or read from a local versioned cache. The integrity mechanisms described below guard against tampering in transit, but not against a compromised upstream release.

verg is a privileged, push-based tool. It is not appropriate to run it with untrusted state files or from an untrusted control host.

---

## Supply chain and agent integrity

When verg needs to push a new agent version to a target, it follows this sequence:

1. **Local cache or download.** verg first looks for the agent binary in a versioned local cache (`agents/<version>/verg-agent-<arch>`). If absent, it downloads from `https://github.com/rvben/verg/releases/download/v<version>/verg-agent-<arch>` using `curl -fSL -m 300`.

2. **Local checksum verification.** Before pushing, verg compares the local binary's SHA-256 against an embedded manifest compiled into the `verg` binary itself. A mismatch is a hard error: the push is refused and the binary is not sent to the target.

3. **Remote checksum verification.** After copying the binary to a temporary path on the target (`/usr/local/bin/verg-agent.tmp.<pid>`), verg runs `sha256sum -c` on the remote host before atomically installing the binary to `/usr/local/bin/verg-agent` with mode 0700. On any failure in this phase, the temp file is removed.

4. **Checksum format guard.** The embedded expected checksum is validated as exactly 64 lowercase hex characters before it is interpolated into a remote shell command, preventing a malformed value from reaching the shell.

**Conditional verification:** Steps 2 and 3 only apply when an embedded checksum is available. A development build with no compiled-in manifest, or a run with `--skip-agent-checksum`, sets `expected` to `None` and skips both local and remote hash checks. `--skip-agent-checksum` is intended for air-gapped environments or locally built binaries where no published release checksum exists.

**Cleanup scope:** The remote temp file is removed on a failure during the remote install phase. It is NOT cleaned up after an `scp` failure (the file may not have been created) or after a local malformed-checksum rejection (the push never starts).

---

## Transport security

All communication between the control host and targets uses SSH.

### Host key checking

The `--host-key-checking` flag controls `StrictHostKeyChecking`. The default is `yes`:

| Value | Behavior |
|-------|----------|
| `yes` (default) | Strict host key checking. Connections to hosts not in the known hosts file are refused. |
| `accept-new` | Trust-on-first-use (TOFU). New hosts are accepted and their key is recorded; changed keys are still rejected. |
| `no` | Host key checking is disabled. **Unsafe.** Allows man-in-the-middle attacks. |

Use `--ssh-known-hosts <file>` to specify an explicit known hosts file (`UserKnownHostsFile`). This is useful when the system known hosts file is not populated.

All SSH connections use `BatchMode=yes`, which disables password prompts and interactive authentication. This ensures verg never stalls waiting for keyboard input and prevents accidental cleartext credential entry.

Connection timeouts are set to `ConnectTimeout=10`, `ServerAliveInterval=15`, and `ServerAliveCountMax=3`.

### SSH option injection prevention

Host `address` and `user` fields are validated before use in SSH commands. Both fields must not start with `-` and may only contain alphanumeric characters plus `. _ : @ - [ ]` (square brackets are permitted to support IPv6 address literals). This prevents a malicious or misconfigured `hosts.toml` from injecting SSH options via crafted field values.

---

## Secret handling

### `sensitive = true`

Marking a resource `sensitive = true` redacts its output in all JSON responses. Specifically:

- The `from`, `to`, and `output` fields are omitted entirely from the JSON (not set to `null`).
- The `diff` field is replaced with `"[redacted]"`.
- `status` and `error` are kept, so you can see whether the resource changed or failed without exposing the payload.

Redaction applies to both the real-time apply output and the changelog.

### `cmd` stdin

The `stdin` property of a `cmd` resource is always treated as sensitive. It is never included in diff output, never echoed in dry-run mode (which instead prints `would run: <command> (with stdin)`), and is omitted from failure error messages.

### Changelog

Every `apply` run writes a structured JSON log to `.verg/logs/<timestamp>-apply.json`. The changelog is designed to record that a resource changed without persisting secret payload bodies:

- The `from`, `to`, and `output` fields are stripped for **all** resources, regardless of whether the resource is marked sensitive.
- The `diff` field is truncated to a 200-byte char-boundary-safe prefix followed by `"..."` if it exceeds that length.
- `sensitive` resources already have their `diff` set to `"[redacted]"` before the changelog is written, so that value is preserved.

The changelog does not encrypt its contents. It avoids persisting payload bodies by stripping them unconditionally.

---

## Safe-by-default behaviors

### Strict config validation

verg validates all state files locally before opening any SSH connections. Unknown top-level keys, unknown resource types, and wrongly-typed special keys are rejected with an `INVALID_CONFIG` error (exit code 5). This catches configuration mistakes before they reach targets.

`--lax-config` downgrades these errors to warnings, allowing partial or transitional configs to be applied. Use with care.

### Apply confirmation

`apply` requires either a terminal (TTY) or the `--yes` flag. Running `verg apply` in a non-interactive pipeline without `--yes` exits with `ConfirmationRequired` (exit code 5). This prevents accidental applies triggered by automation that did not explicitly opt in.

### Explicit `--targets` required for apply

`apply` has no default target. `--targets` is always required, which prevents accidentally applying to all hosts when the intent was to target a subset.

### Secure PATH for remote commands

All commands executed by the agent use a fixed `PATH`:

```
/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
```

This prevents PATH-hijacking attacks where a user-controlled directory earlier in `PATH` could shadow system binaries used by resource executors.

### Bounded agent stdin

The agent reads its input bundle from stdin with a hard 64 MiB cap. If the bundle exceeds this limit the agent exits with code 5. This prevents out-of-memory conditions caused by unexpectedly large payloads.

### Download extraction containment

When a `download` resource uses `extract = true`:

- A `checksum` (SHA-256 hex) is mandatory. The checksum is verified before extraction begins, so a corrupt or tampered archive is rejected before any files are written.
- After extraction, each extracted file path is checked for symlink-escape: extracted paths that resolve outside the destination directory are rejected.
- `tar` extraction uses hardening flags: `--no-same-owner`, `--no-same-permissions`, `--no-overwrite-dir`.
- A scoped temporary directory is used and cleaned up on completion or failure.

### Cron injection guards

The `cron` resource writes to `/etc/cron.d/<name>`. To prevent injection attacks via crafted field values:

- The cron entry `name` is restricted to `[a-zA-Z0-9_-]` characters only.
- Schedule fields are validated: five fields, using only `[0-9*/,-]`, with per-field range maxima (minutes 0-59, hours 0-23, day-of-month 0-31, month 1-12, day-of-week 0-7).
- The `user`, `mailto`, `env` values, and `command` are checked for control characters and newlines.

---

## Reporting a vulnerability

Please do **not** open a public GitHub issue to report a security vulnerability.

Instead, either:

- Email the maintainer directly at **ruben.jongejan@gmail.com**, or
- Use **GitHub Security Advisories** to report privately: go to the [rvben/verg repository](https://github.com/rvben/verg) and open a private advisory via the Security tab.

Please include a description of the vulnerability, steps to reproduce it, and the version of verg you tested against.

verg is a pre-1.0 project maintained by a single person. There is no formal SLA for security response, but reports will be read and addressed as quickly as circumstances allow.

---

## Supported versions

verg is pre-1.0. Only the **latest released version** receives security fixes. There are no backport branches. If you are running an older version, upgrade to the latest release.

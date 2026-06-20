# Resource Reference

A **resource** is a desired-state declaration for a single manageable item on a target host. Resources are declared as TOML tables with the shape `[resource.<type>.<name>]`, where `<type>` is one of the 11 types listed below and `<name>` is a free-form identifier (letters, numbers, hyphens, underscores). The fully qualified name (FQN) `type.name` is the stable identifier used in `after` dependency lists and `notify` handler references.

---

## Common attributes

Every resource type accepts the following attributes in addition to its own properties.

### after

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `after` | array of strings | none | FQNs (`type.name`) of resources that must complete before this one runs |

Resources are sorted into execution layers using Kahn's topological sort. Within a layer, resources run in FQN order. An unknown FQN is a config error. A cycle is a config error. A resource whose dependency fails is skipped with the reason "dependency failed".

```toml
[resource.service.app]
name = "myapp"
after = ["pkg.myapp", "file.myapp-conf"]
```

### notify and handlers

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `notify` | string or array | none | Handlers or shorthand actions to trigger when this resource changes |
| `handler` | bool | `false` | When `true`, this resource is skipped unless notified by another resource |

A resource with `handler = true` is only executed when another resource lists its FQN in `notify`. Handlers bypass the guard requirement for `cmd` resources. Handlers form their own DAG and run after all normal resources complete; shorthand actions run after handlers.

`notify` accepts handler FQNs as well as the following shorthand action forms:

| Shorthand | Effect |
|-----------|--------|
| `daemon-reload` | `systemctl daemon-reload` |
| `restart:<svc>` | `systemctl restart <svc>` |
| `reload:<svc>` | `systemctl reload <svc>` |
| `docker-restart:<path>` | `docker compose -f <path>/docker-compose.yml restart` (`<path>` is the project directory) |
| `docker-up:<path>` | `docker compose -f <path>/docker-compose.yml up -d` |
| `docker:<path>` | alias for `docker-restart:<path>` |
| bare service name | equivalent to `restart:<name>` |

Each shorthand runs at most once per host per agent run (deduped within a single agent process). On a multi-host apply, each host runs it independently.

```toml
[resource.file.nginx-conf]
path = "/etc/nginx/nginx.conf"
source = "files/nginx.conf"
notify = ["reload:nginx"]

[resource.cmd.reload-nginx]
command = "nginx -s reload"
creates = "/run/nginx.pid"
handler = true
```

### when

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `when` | string | none | Conditional expression; resource is skipped when it evaluates to false |

Expression operators: `==`, `!=`, `&&`, `||`, `!`. `||` binds looser than `&&`. Parentheses are not supported. A missing fact in a `==` or `!=` comparison evaluates to false (the resource is skipped).

Available operands:

| Operand | Value |
|---------|-------|
| `fact.os` | OS identifier from `/etc/os-release` (e.g. `ubuntu`) |
| `fact.os_release` | Distribution codename (e.g. `noble`) |
| `fact.os_version` | Version ID (e.g. `24.04`) |
| `fact.arch` | Architecture from `uname -m` (e.g. `x86_64`) |
| `fact.hostname` | Remote hostname |
| `group.<name>` | `"true"` when the host belongs to group `<name>` |

```toml
[resource.pkg.snapd]
name = "snapd"
when = "fact.os == 'ubuntu' && fact.os_version != '24.04'"
```

### register

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `register` | string | none | Capture trimmed stdout from a `cmd` resource into a named register |

A `cmd` resource with `register` does not require a guard. Downstream resources access the captured value as `{{ register.NAME }}`. The producer must appear in the consumer's `after` list (or build fails). Register names must be unique. Stdout is trimmed and capped at 64 KiB.

```toml
[resource.cmd.get-version]
command = "cat /opt/app/VERSION"
register = "app_version"

[resource.file.version-marker]
path = "/var/lib/app/installed-version"
content = "{{ register.app_version }}"
after = ["cmd.get-version"]
```

### template

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `template` | bool | `false` | Render `source`, `compose_file`, and `env_file` file contents through the Jinja2 template engine |

Inline `content` is always interpolated regardless of this flag. `register` references in templates are protected through the render phase.

```toml
[resource.file.app-conf]
path = "/etc/app/config.ini"
source = "templates/config.ini.j2"
template = true
```

### sensitive

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `sensitive` | bool | `false` | Redact resource details from output and the changelog |

When `true`, `from`, `to`, and `output` fields are omitted entirely from JSON (not emitted as null), and `diff` is set to `"[redacted]"`. Status and error fields are preserved.

```toml
[resource.cmd.set-password]
command = "chpasswd"
stdin = "admin:{{ admin_password }}"
unless = "false"
sensitive = true
```

### vars

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `vars` | object | none | Per-resource scalar property defaults, applied after template interpolation |

`vars` supplies fallback values for resource properties (scalars only; tables and arrays are dropped). It does NOT inject values into `{{ ... }}` template expressions. Existing properties and host-level variables take precedence over `vars`.

```toml
[resource.file.motd]
path = "/etc/motd"
content = "Welcome to {{ env_name }}"

[resource.file.motd.vars]
env_name = "production"
```

---

## pkg

Install or remove system packages. Auto-detects the package manager: `apt-get` is tried first, then `dnf`, then `pacman`.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | one of `name`/`names` | - | A single package name |
| `names` | array of strings | one of `name`/`names` | - | Multiple package names |
| `state` | string | no | `"present"` | `"present"` to install, `"absent"` to remove |

Provide `name` for a single package or `names` for several. Supplying neither is an error. When both are present, `names` takes precedence and `name` is ignored.

**Behavior notes:**
- On Apt systems, installation status is checked using `dpkg -s` and requires the line `Status: install ok installed` in the output. Exit code alone is insufficient because `dpkg -s` exits 0 for packages that have been removed but retain config files.
- The package cache (`apt-get update -qq`) is refreshed at most once per run, only when at least one package needs to be installed.

```toml
[resource.pkg.jq]
name = "jq"
state = "present"
```

---

## file

Manage a file's content, permissions, and ownership.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `path` | string | yes | - | Absolute path to the file on the target |
| `content` | string | no | - | Inline file content (always interpolated as a template) |
| `source` | string | no | - | Path relative to the verg project directory; content is resolved at build time |
| `mode` | string | no | - | Permissions in octal notation (e.g. `"0644"`) |
| `owner` | string | no | - | Username or UID to own the file |

`content` and `source` are mutually optional; omitting both manages only mode and owner without touching file content. Writes are atomic (write to a temp file, then rename). Owner is detected via `ls -ld`.

**Behavior notes:**
- `source` is resolved to file content at build time on the control machine. The agent receives the content directly; there is no agent-side source path lookup.
- If `template = true`, the content loaded from `source` is rendered through the Jinja2 engine before being written. Inline `content` is always rendered.
- Parent directories are created automatically if missing when writing content.

```toml
[resource.file.greeting]
path = "/tmp/verg-test.txt"
content = "{{ greeting }}"
mode = "0644"
```

---

## directory

Ensure a directory exists with the desired permissions and ownership.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `path` | string | yes | - | Absolute path to the directory on the target |
| `owner` | string | no | - | Username or UID |
| `group` | string | no | - | Groupname or GID |
| `mode` | string | no | - | Permissions in octal notation (e.g. `"0755"`) |
| `recurse` | bool | no | `false` | Apply ownership changes recursively (`chown -R`) |
| `state` | string | no | `"present"` | `"present"` to ensure it exists, `"absent"` to remove it |

**Behavior notes:**
- `state = "absent"` removes the directory and all its contents (`rm -rf` equivalent).
- When both `owner` and `group` drift, a single `chown owner:group` call is issued. Group drift triggers a `chown` even when the owner already matches.
- Ownership comparison uses numeric UID/GID when the provided value is numeric; otherwise compares by name via `ls -ld`.

```toml
[resource.directory.app-data]
path = "/var/lib/myapp"
owner = "myapp"
group = "myapp"
mode = "0750"
recurse = false
```

---

## service

Manage the running state and boot enablement of a systemd service.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | yes | - | Systemd unit name (e.g. `"nginx"` or `"nginx.service"`) |
| `state` | string | no | `"running"` | `"running"` to start, `"stopped"` to stop |
| `enabled` | bool | no | unmanaged | Whether the service starts on boot; omit to leave boot enablement unchanged |

**Behavior notes:**
- `enabled` is a three-state attribute: `true` enables, `false` disables, and omitting it leaves the current boot setting untouched.
- The enabled check requires the `systemctl is-enabled` output to be exactly `"enabled"` or `"enabled-runtime"`. Units reported as `"static"`, `"indirect"`, `"alias"`, or `"generated"` are treated as not enabled, because those states cannot be toggled by `systemctl enable/disable`.

```toml
[resource.service.nginx]
name = "nginx"
state = "running"
enabled = true
```

---

## cmd

Run an arbitrary shell command with an idempotency guard.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `command` | string | yes | - | Shell command to run (via `sh -c`) |
| `creates` | string | no | - | Skip if this path exists |
| `unless` | string | no | - | Skip if this command exits 0 |
| `onlyif` | string | no | - | Run only if this command exits 0 |
| `stdin` | string | no | - | Data piped to the command's stdin; never echoed in diffs or output |
| `register` | string | no | - | Capture trimmed stdout into a named register (see common attributes) |

At least one of `creates`, `unless`, `onlyif`, or `register` is required unless the resource is a handler (`handler = true`).

**Behavior notes:**
- Guards are checked in order: `creates`, then `unless`, then `onlyif`. The first matching guard that causes a skip short-circuits the rest.
- `stdin` is treated as sensitive: dry-run diffs show `"would run: <command> (with stdin)"` without revealing the content.
- `register` satisfies the guard requirement. A `cmd` with `register` always runs (no guard needed) and captures trimmed stdout, capped at 64 KiB.
- On failure, the command text is omitted from the error message to avoid leaking context.

```toml
[resource.cmd.marker]
command = "echo 'verg was here' > /tmp/verg-marker"
creates = "/tmp/verg-marker"
```

---

## user

Create or remove a system user.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | yes | - | Username to create or remove |
| `state` | string | no | `"present"` | `"present"` to create, `"absent"` to remove |
| `home` | string | no | - | Home directory path (`useradd -d <home> -m`) |
| `shell` | string | no | - | Login shell (`useradd -s <shell>`) |
| `groups` | string | no | - | Supplementary groups, comma-separated (`useradd -G <groups>`) |

**Behavior notes:**
- Users are always created as system users (`useradd --system`).
- `state = "absent"` runs `userdel -r`, removing the user's home directory and mail spool.
- This resource only manages user existence. If the user already exists, no attributes (home, shell, groups) are updated. To change attributes of an existing user, remove and re-create it or use a `cmd` resource.

```toml
[resource.user.deploy]
name = "deploy"
state = "present"
home = "/home/deploy"
shell = "/bin/bash"
groups = "docker,sudo"
```

---

## cron

Manage a cron job file in `/etc/cron.d/<name>` with mode 0644.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | yes | - | Cron file name; only `[a-zA-Z0-9_-]` is allowed |
| `user` | string | required when `state = "present"` | - | User to run all jobs as; applies to every job in the file |
| `schedule` | string | one of single/multi form | - | Cron schedule (5 fields); single-job form |
| `command` | string | one of single/multi form | - | Command to run; single-job form |
| `jobs` | array of objects | one of single/multi form | - | Multiple jobs (multi-job form); each object has `schedule` and `command` |
| `mailto` | string | no | - | `MAILTO` value written into the cron file |
| `env` | object | no | - | Additional environment variables written into the cron file |
| `state` | string | no | `"present"` | `"present"` to create/update, `"absent"` to remove the file |

Use either the single-job form (`schedule` + `command`) or the multi-job form (`jobs` array), not both. The `jobs` array must not be empty.

**Behavior notes:**
- `name` is used directly as the filename. Path separators, spaces, and other special characters are rejected to prevent path traversal.
- `user` is required when `state = "present"`. The top-level `user` applies to all jobs. A `user` key inside a `jobs[].` entry is accepted by the schema but is silently ignored at runtime.
- Schedule validation: must be exactly 5 whitespace-separated fields; each field may contain only `[0-9*/,-]`; numeric values must not exceed field maxima (minute: 59, hour: 23, day: 31, month: 12, weekday: 7).
- Injection guards: `user`, `mailto`, `env` keys/values, and `command` are checked for control characters and newlines.
- The cron file is written atomically. If content is already correct but mode has drifted, only the mode is corrected.

```toml
[resource.cron.zfs-backup]
name = "zfs-backup"
user = "root"
schedule = "0 3 * * *"
command = "/root/backup.sh"
mailto = ""
```

Multi-job example:

```toml
[resource.cron.hours-automation]
name = "hours-automation"
user = "root"
mailto = ""

[[resource.cron.hours-automation.jobs]]
schedule = "0 20 * * 1-4"
command = "/opt/hours-automation/run.sh"

[[resource.cron.hours-automation.jobs]]
schedule = "0 20 * * 5"
command = "/opt/hours-automation/run.sh --close-week"
```

---

## sysctl

Set a Linux kernel parameter at runtime and optionally persist it across reboots.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `key` | string | yes | - | Sysctl key (e.g. `net.ipv4.ip_forward`) |
| `value` | string | yes | - | Desired value |
| `persist` | bool | no | `false` | Write to `/etc/sysctl.d/99-verg.conf` for persistence across reboots |

**Behavior notes:**
- The runtime value is set immediately with `sysctl -w key=value`.
- When `persist = true`, `/etc/sysctl.d/99-verg.conf` is updated atomically. The file is rebuilt preserving all comments and all other keys; only the target key is replaced (or appended). Space-normalized comparison (`key=val` and `key = val` both match) prevents spurious writes.

```toml
[resource.sysctl.ip-forward]
key = "net.ipv4.ip_forward"
value = "1"
persist = true
```

---

## apt_repo

Add or remove an APT repository with its GPG signing key.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | yes | - | Repository identifier; used as the filename stem |
| `url` | string | yes | - | Base URL of the repository |
| `gpg_key` | string | yes | - | URL to the GPG signing key |
| `suite` | string | no | auto-detected | Distribution suite (e.g. `noble`); auto-detected via `lsb_release -cs` if omitted |
| `component` | string | no | `"stable"` | Repository component |
| `arch` | string | no | `"amd64"` | Package architecture |
| `state` | string | no | `"present"` | `"present"` to add, `"absent"` to remove |

**Behavior notes:**
- The GPG key is saved to `/etc/apt/keyrings/<name>.asc` and the source list to `/etc/apt/sources.list.d/<name>.list`.
- The key file is only downloaded when absent. To rotate a signing key, apply the resource with `state = "absent"` first, then re-apply with `state = "present"`.
- `apt-get update -qq` is run automatically after adding or updating the source list.
- `state = "absent"` removes both the keyring file and the source list file.

```toml
[resource.apt_repo.docker]
name = "docker"
url = "https://download.docker.com/linux/ubuntu"
gpg_key = "https://download.docker.com/linux/ubuntu/gpg"
component = "stable"
```

---

## docker_compose

Manage a Docker Compose stack on the target using `docker compose` (v2).

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `project_dir` | string | yes | - | Directory on the target where the compose project lives |
| `compose_file` | string | no | - | Path to a compose file relative to the verg project directory; content is resolved at build time |
| `env_file` | string | no | - | Path to a `.env` file relative to the verg project directory; content is resolved at build time |
| `state` | string | no | `"up"` | `"up"` to start the stack, `"down"` to stop it |
| `pull` | bool | no | `true` | Pull images before starting |

**Behavior notes:**
- `compose_file` and `env_file` are resolved to their file contents on the control machine at build time.
- `docker-compose.yml` is written to `<project_dir>/docker-compose.yml` only when `compose_file` is provided (i.e. only when content is present in the bundle).
- `.env` is written to `<project_dir>/.env` only when `env_file` is provided.
- `state = "up"` runs `docker compose up -d --remove-orphans`. `state = "down"` runs `docker compose down`.
- If `template = true`, the content loaded from `compose_file` and `env_file` is rendered through the Jinja2 engine.

```toml
[resource.docker_compose.monitoring]
project_dir = "/opt/monitoring"
compose_file = "files/monitoring/docker-compose.yml"
env_file = "files/monitoring/.env"
state = "up"
pull = true
```

---

## download

Download a file from a URL and place it at a destination path, optionally extracting an archive.

| Property | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `url` | string | yes | - | URL to download from |
| `dest` | string | yes | - | Destination path on the target |
| `state` | string | no | `"present"` | `"present"` to download, `"absent"` to remove |
| `checksum` | string | no | - | Expected SHA256 hex digest; required when `extract = true` |
| `extract` | bool | no | `false` | Extract the archive and install the file matching `dest`'s basename |
| `mode` | string | no | - | File permissions in octal notation (e.g. `"0755"`) |
| `owner` | string | no | - | File owner |

**Behavior notes:**
- `extract = true` requires `checksum`. Extraction without a pinned checksum is rejected before any network activity.
- Supported archive formats for extraction: `.zip` and `.tar.gz`/`.tgz`. After extraction, verg looks for a file whose name matches the basename of `dest`; if none is found, the error lists the files that were found.
- Extraction uses hardening flags: `tar --no-same-owner --no-same-permissions --no-overwrite-dir`. Symlink-escape containment is enforced after extraction.
- Direct download (no extraction) uses `curl -fSL` with no timeout or max-filesize limit.
- Extract-mode download uses `curl -fSL -m 300 --max-filesize 536870912` (5 min, 512 MiB).
- When `checksum` is provided for a direct download, the checksum is verified after download; a mismatch removes the file and fails the resource.
- If the file already exists and a `checksum` is provided, the existing file is verified. A mismatch triggers a re-download.

```toml
[resource.download.kubectl]
url = "https://dl.k8s.io/release/v1.30.0/bin/linux/amd64/kubectl"
dest = "/usr/local/bin/kubectl"
mode = "0755"
checksum = "b4b6a853ebf9fb24bbc7f2028855c52d9d64e09b4f9f0edeb23b4be4e4572a03"
```

Archive extraction example:

```toml
[resource.download.mytool]
url = "https://releases.example.com/mytool-1.0.tar.gz"
dest = "/usr/local/bin/mytool"
extract = true
mode = "0755"
checksum = "a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4"
```

# Native Providers

A **native provider** is a custom resource type implemented as a script (any language) that communicates with verg over a JSON-over-stdio protocol. Unlike declarative `resource_def`s (which express idempotency as a `check` shell command + an `apply` shell command and receive params as environment variables), a native provider receives a single structured JSON request on stdin and writes a single structured JSON response to stdout. The provider script is responsible for both checking and converging state in one call per resource instance, and for reporting whether a change was made.

Key differences from `resource_def`s:

| | `resource_def` | Native provider |
|---|---|---|
| Idempotency model | Separate `check` and `apply` scripts | Single script that handles `"plan"` and `"apply"` actions |
| Params delivery | Environment variables (`VERG_PARAM_*`) | JSON `params` field on stdin |
| Source location | Inline in the definition TOML | Separate script file, embedded in the bundle at build time |
| Language | Shell only | Any language whose interpreter is available on the target |

Because the source script is **embedded into the bundle at build time** on the control host, pull-mode agents (those that fetch pre-built bundles via `verg publish`) are fully self-contained and do not need access to the original script file. The **interpreter** (e.g. `/usr/bin/python3`, `/bin/sh`) must be present on every target that will run the provider.

---

## Declaring a Provider

Place one or more `.toml` files in `verg/providers/`. Each file can contain one or more provider definitions. A missing `verg/providers/` directory is fine - no providers are loaded, which is not an error.

```toml
[provider.<type>]
description = "What this provider does"
interpreter = ["/usr/bin/python3"]
source      = "providers/my_script.py"

[provider.<type>.params]
param_name = { type = "string", required = true }
```

`<type>` becomes the resource type name used in state files (`[resource.<type>.<name>]`). Non-`.toml` files in `verg/providers/` are silently ignored. Files are loaded in lexicographic order by filename.

### Provider declaration fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | string | no | Human-readable description of what this provider manages |
| `interpreter` | array of strings | yes | Argv prefix for running the script; must be non-empty |
| `source` | string | yes | Path to the script file, relative to the verg project directory |
| `params` | table | no | Named parameters that instances of this type accept |

`interpreter` is an argv array, not a shell string. `["/bin/sh", "-e"]` passes `-e` as a separate argument to the shell before the script path. The script path is appended by verg as the last argument; the interpreter must accept the script as a positional argument.

`source` is read from disk at bundle-build time and the file's text is embedded in the bundle. The file at `source` must exist on the control host when `verg apply`, `verg diff`, `verg check`, or `verg publish` is run.

### Param schema

Each entry under `[provider.<type>.params]` follows the same schema as `resource_def` params:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `type` | string | `"string"` | One of `string`, `integer`, `float`, `boolean` |
| `required` | bool | `false` | Reject an instance that omits this param |
| `default` | literal | none | Fallback value when the instance omits this param; must match `type` |
| `enum` | array of strings | none | For `string` params: restrict allowed values to this list |

Param names must match `[a-zA-Z_][a-zA-Z0-9_]*`. The following names are reserved and cannot be used: `after`, `notify`, `when`, `handler`, `template`, `register`, `vars`, `sensitive`, `source`, `compose_file`, `env_file`.

### Namespace constraints

Provider type names must not collide with:
- Built-in resource types (`pkg`, `file`, `service`, `cmd`, `user`, `cron`, `sysctl`, `apt_repo`, `docker_compose`, `download`, `directory`, etc.)
- Any custom `resource_def` type defined in `verg/resources/`
- Another provider type defined in any other file in `verg/providers/`

All three checks happen at load time. A collision is a config error (exit code 5).

---

## Wire Protocol

When a provider resource is executed, verg:

1. Writes the embedded source text to a temporary file (mode `0600`, path `<tmpdir>/verg-provider-<pid>-<seq>`) on the target.
2. Runs `interpreter[0] interpreter[1]... <tempfile>`, piping a JSON request to stdin.
3. Reads the JSON response from stdout.
4. Removes the temporary file (the guard removes it even on error).

Params that are TOML datetime values are serialized as plain ISO-8601 strings in the JSON request, not as tagged objects. String param values are passed through literally with no interpolation or environment expansion.

### Request

verg writes exactly one JSON object to the script's stdin:

```json
{
  "protocol_version": 1,
  "action": "plan",
  "type": "my_provider",
  "name": "my-instance",
  "params": {
    "zone": "example.com",
    "ttl": 300
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `protocol_version` | integer | Always `1` |
| `action` | string | `"plan"` during dry-run (`verg diff`/`verg check`); `"apply"` during `verg apply` |
| `type` | string | The provider type name (matches `[provider.<type>]`) |
| `name` | string | The resource instance name (from `[resource.<type>.<name>]`) |
| `params` | object | Resolved param values: instance values take precedence, and params with a declared `default` that the instance omits are sent with their default value; params with no instance value and no default are absent |

### Response

The script must write exactly one JSON object to stdout before exiting:

```json
{
  "status": "changed",
  "diff": "created record A 192.0.2.10",
  "output": "record-id-42",
  "error": null
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `status` | string | yes | One of `"ok"`, `"changed"`, or `"failed"` |
| `diff` | string | no | Human-readable description of what changed (or what would change for `"plan"`); shown in `verg diff`/`verg apply` output |
| `output` | string | no | Captured output available to downstream resources via `register`; only used when status is not `"failed"` |
| `error` | string | no | Error message; used when `status` is `"failed"` |

### Status values

| Status | Meaning |
|--------|---------|
| `"ok"` | Already in desired state; no change was made |
| `"changed"` | A change was made (`"apply"`) or drift was detected (`"plan"`) |
| `"failed"` | The provider encountered an error |

An unknown status value (anything other than the three above) is treated as `"failed"`.

### Exit codes and error semantics

| Condition | Result |
|-----------|--------|
| Exit 0 and valid JSON with known status | Response is authoritative |
| Exit non-zero | `"failed"` - stderr is included in the error message |
| Exit 0 but stdout is not valid JSON | `"failed"` - parse error is reported |
| Exit 0 and JSON with unknown status | `"failed"` - the unknown status value is reported |

Protocol violations (non-zero exit, unparseable stdout, unknown status) are reported as a `"failed"` result for that resource. They do not abort the rest of the run.

---

## Plan vs. Apply

When verg runs in dry-run mode (`verg diff` or `verg check`), it sends `"action": "plan"`. When applying (`verg apply`), it sends `"action": "apply"`.

**The provider must not mutate any real state when `action` is `"plan"`.** A plan call checks whether drift exists and returns `"ok"` (no drift) or `"changed"` (drift detected). Only an `"apply"` call should make actual changes.

A `"changed"` response to a `"plan"` action means "drift was detected; running apply would change this". If no `diff` is provided, verg uses the default message `"would change"` for plan and `"changed"` for apply.

---

## Output and Register

The `output` field in the response feeds the `register` attribute on the resource instance. This works identically to `cmd` resources: downstream resources can reference the value as `{{ register.NAME }}`.

`output` is only captured when `status` is `"ok"` or `"changed"`. When `status` is `"failed"`, the `output` field is discarded even if present.

```toml
[resource.my_provider.get-record]
zone   = "example.com"
record = "web1"
register = "record_ip"

[resource.file.record-marker]
path    = "/var/lib/app/record-ip"
content = "{{ register.record_ip }}"
after   = ["my_provider.get-record"]
```

---

## Common Resource Attributes

Provider resource instances support all common resource attributes: `after`, `when`, `notify`, `register`, `sensitive`, `handler`, `template`, `vars`. They participate in the same DAG and handler system as built-in resources.

---

## Worked Example

This example shows a `/bin/sh` provider that manages a marker file at a given path. It is intentionally simple to illustrate the protocol without requiring any external tool.

### 1. Write the script: `providers/marker.sh`

```sh
#!/bin/sh
# Reads a JSON request on stdin and writes a JSON response to stdout.
# Fields extracted with grep/sed for portability - replace with jq if available.
set -eu

req=$(cat)

# Extract the "action" and "path" param from the JSON.
action=$(printf '%s' "$req" | sed -n 's/.*"action"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
path=$(printf '%s' "$req" | sed -n 's/.*"path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

if [ "$action" = "plan" ]; then
    if [ -f "$path" ]; then
        printf '%s' '{"status":"ok"}'
    else
        printf '%s' '{"status":"changed","diff":"would create '"$path"'"}'
    fi
elif [ "$action" = "apply" ]; then
    if [ -f "$path" ]; then
        printf '%s' '{"status":"ok"}'
    else
        touch "$path"
        printf '%s' '{"status":"changed","diff":"created '"$path"'"}'
    fi
else
    printf '%s' '{"status":"failed","error":"unknown action: '"$action"'"}'
fi
```

### 2. Declare the provider: `providers/marker.toml`

```toml
[provider.marker]
description = "Ensure a marker file exists at the given path"
interpreter = ["/bin/sh"]
source      = "providers/marker.sh"

[provider.marker.params]
path = { type = "string", required = true }
```

### 3. Use it in a state file: `state/base.toml`

```toml
[resource.marker.web-ready]
path = "/var/lib/app/ready"
```

### 4. Preview and apply

```sh
# Preview (plan only - the script receives action="plan" and must not write the file)
verg diff --targets all

# Apply (the script receives action="apply" and creates the file if absent)
verg apply --targets all
```

---

## Notes and Pitfalls

**Interpreter must be present on the target.** The source script travels in the bundle; the interpreter does not. If `/usr/bin/python3` is listed as the interpreter but is not installed on a target, the provider will fail on that target with a non-zero exit from the interpreter launcher.

**Use strings for timestamps.** TOML datetime values (`2024-01-02T03:04:05Z`) are serialized as plain ISO-8601 strings in the JSON request. Do not rely on a specific TOML datetime type in your script; read the param value as a string.

**Sensitive params.** Params travel on stdin only, never as command-line arguments or in log output. For resources whose params contain secrets, set `sensitive = true` on the resource instance to redact the result from output and the changelog.

**Script runs as data, not via the exec bit.** The embedded source is written to a temp file and passed as an argument to the interpreter. The file does not need to be executable and works even on filesystems mounted `noexec`.

**Schema integration.** `verg schema` includes provider types under `resource_types` with a `"provider": true` marker and the full param schema. The schema is loaded from `verg/providers/` at the time `verg schema` runs.

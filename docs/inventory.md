# Inventory

verg reads its host inventory from `verg/hosts.toml`. Hosts can be declared
statically as `[hosts.NAME]` tables, generated dynamically by a command, or
both.

## Static hosts

```toml
[hosts.web1]
address = "192.0.2.10"
user    = "deploy"      # optional, defaults to "root"
port    = 2222          # optional
groups  = ["web", "prod"]

[hosts.web1.vars]
role = "frontend"
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `address` | yes | - | IP address or hostname |
| `user` | no | `"root"` | SSH user |
| `port` | no | `22` | SSH port |
| `groups` | no | `[]` | Group memberships |
| `[hosts.NAME.vars]` | no | - | Host-specific variables |

## Dynamic inventory

An `[inventory]` section runs a command on the **control host** (the machine
running `verg`) and parses its output into hosts. This lets verg pull hosts
from a cloud provider, a CMDB, or any script.

```toml
[inventory]
command = ["./scripts/list-hosts", "--env", "prod"]
```

`command` is an argv array. verg executes it directly with no shell, so there
is no quoting or interpolation: each array element is one argument. The first
element is the program (looked up on `PATH` if not an absolute path). The
command runs with its working directory set to the verg config directory (the
one containing `hosts.toml`), so a relative path such as `./scripts/list-hosts`
resolves against your project, not the directory you happened to run verg from.

### Output format

The command must print a JSON object to stdout, keyed by host name. Each value
takes the same fields as a static host:

```json
{
  "web1": {
    "address": "192.0.2.10",
    "user": "deploy",
    "port": 2222,
    "groups": ["web", "prod"],
    "vars": { "role": "frontend" }
  },
  "db1": {
    "address": "192.0.2.5"
  }
}
```

Only `address` is required. `user` defaults to `"root"`, `groups` defaults to
an empty list, and `port` and `vars` may be omitted - exactly as for static
hosts. `vars` values must be representable as TOML values; JSON `null` is
rejected because TOML has no null. `address` and `user` are validated the same
way as static hosts (they may not start with `-` and are restricted to a safe
character set).

### Merging with static hosts

Static `[hosts.*]` entries and dynamic hosts are combined into one inventory.
Dynamic hosts join the same groups and receive the same group variables as
static hosts. If a host name is defined both statically and by the inventory
command, verg stops with an error rather than silently picking one - rename one
of them.

A `hosts.toml` may contain only `[inventory]` (no static `[hosts.*]`), only
static hosts, or both.

### Failure handling

verg fails the run (exit code 5) when:

- the command cannot be spawned (for example, the program is not found),
- the command exits non-zero (its stderr is included in the error),
- the output is not valid UTF-8 or not a valid JSON host map,
- a dynamic host has an invalid `address` or `user`,
- a host name collides between static and dynamic inventory.

## Selectors

The `--targets` flag accepts a selector expression over the combined inventory:

| Syntax | Meaning |
|--------|---------|
| `all` | Every host |
| `web` | Hosts named `web` or in group `web` |
| `a,b` | Union of selectors `a` and `b` |
| `a:b` | Intersection (hosts in both `a` and `b`) |
| `!x` | Exclude `x` |
| `prod:!db` | In group `prod` but not group `db` |

An unknown selector name is an error (exit code 6). The one exception is an
exclusion (`!x`): excluding a group or host that matches nothing excludes
nothing rather than erroring, so `prod:!down` still works when the `down` group
is empty (a misspelled exclusion is therefore silently a no-op). Parentheses are
not supported.

# Secrets

verg supports encrypted secrets via [age](https://age-encryption.org/). Secrets are encrypted at rest and decrypted on the **control host** at bundle-build time. The decrypted values are injected into the template context under the `secret.*` namespace, so templates can reference them just like any other variable.

A project with no `verg/secrets.age` file behaves exactly as before; the feature is opt-in by the file's presence.

## Prerequisites

The `age` CLI must be installed on the **control host** (the machine that runs `verg apply`, `verg diff`, `verg check`, or `verg publish`). Targets do not need `age`; they receive already-rendered values inside the encrypted SSH transport.

Install age on the control host:

```sh
# macOS
brew install age

# Debian/Ubuntu
apt-get install age

# Or download a release binary from https://github.com/FiloSottile/age/releases
```

## Creating a Secrets File

### 1. Generate an identity (private key)

```sh
age-keygen -o key.txt
```

`key.txt` is your identity file. It contains the private key. Keep it safe and do **not** commit it.

### 2. Derive the recipient (public key)

```sh
age-keygen -y key.txt
```

This prints a line like `age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p`. Copy it.

### 3. Write a plaintext secrets file

Create `secrets.toml` with your secret values. Nested tables are supported:

```toml
app_token = "s3cr3t"

[db]
password = "hunter2"
```

### 4. Encrypt it

```sh
age --encrypt -r age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p \
    -o verg/secrets.age secrets.toml
```

Replace the `age1...` value with the recipient printed in step 2.

### 5. Commit only the encrypted file

```sh
git add verg/secrets.age
# Do NOT add key.txt or secrets.toml
```

`verg/secrets.age` is safe to commit. The plaintext `secrets.toml` and the identity `key.txt` must never be committed.

## Using Secrets in Templates

Any string property or template file can reference secrets via `{{ secret.<name> }}`:

```toml
[resource.file.app-token]
path = "/etc/app/token"
content = "{{ secret.app_token }}"
sensitive = true
```

Nested tables in `secrets.toml` become nested namespace access:

```toml
[resource.file.db-pass]
path = "/etc/app/db.conf"
content = "password={{ secret.db.password }}"
sensitive = true
```

Always pair a resource that uses a secret value with `sensitive = true`. See the Security section below.

## Providing the Identity

Tell verg where to find the identity file via a flag or environment variable:

```sh
# Flag (takes a file path)
verg apply --targets all --age-identity /path/to/key.txt

# Environment variable
export VERG_AGE_IDENTITY=/path/to/key.txt
verg apply --targets all
```

`--age-identity` is a global flag, so it works with `apply`, `diff`, `check`, and `publish`.

If `verg/secrets.age` exists but no identity is provided, verg exits with code 5 and a clear error message:

```
verg/secrets.age exists but no age identity was provided (set --age-identity or VERG_AGE_IDENTITY)
```

If the `age` binary is not found on the control host, verg also exits with code 5:

```
failed to run age (is the age CLI installed on the control host?): ...
```

## Reserved Template Name

`secret` is a **reserved** top-level template name. A host variable literally named `secret` (in `[hosts.NAME.vars]` or a group vars file) would shadow the secrets namespace. Do not use `secret` as a variable name in your inventory.

## Security Model

### Pair secrets with `sensitive = true`

Mark every resource that renders a secret with `sensitive = true`:

```toml
[resource.file.app-token]
path    = "/etc/app/token"
content = "{{ secret.app_token }}"
sensitive = true
```

When `sensitive = true`, redaction happens **on the agent, before the result is returned to the control host**: the `from`, `to`, and `output` fields are cleared and, if a diff is present, it is replaced with `"[redacted]"`. Because this happens before the result leaves the agent, the redacted values are what appear in BOTH the live JSON output AND the persisted apply changelog. Status and error fields are preserved so you can still see whether the resource changed or failed.

Independently, the changelog (`.verg/logs/*.json`) strips `from`, `to`, and `output` for **all** resources regardless of `sensitive`, and truncates long diffs to 200 bytes. So a non-sensitive resource never persists its payload either; a sensitive resource shows `"[redacted]"` for its diff (already redacted by the agent) rather than a truncated value.

Without `sensitive = true`, the rendered secret value can appear in `verg apply` output and in `diff`.

### Decryption happens on the control host

verg calls `age --decrypt` on the control machine before building bundles. The decrypted value is then rendered into resource properties and travels inside the bundle. The bundle is sent to the target over SSH (an encrypted channel), so the value is protected in transit.

On the target, the decrypted value is necessarily present because it is the configuration you are writing. Mark the destination file with appropriate permissions (`mode = "0600"`, `owner = "root"`) to limit who can read it once it is in place.

### Published bundles contain plaintext secrets

`verg publish` writes **rendered** bundles to disk. A rendered bundle contains the decrypted secret value in plaintext inside the TOML file. This is unavoidable: the pull-mode agent must be able to read the bundle without any cryptographic capability.

Treat published bundle files as secret material:

- Restrict the destination directory (`chmod 700` or equivalent).
- If you serve bundles over HTTP, use HTTPS and restrict access so each host can only fetch its own bundle.
- Never commit the `--dest` directory to version control.

See [docs/continuous-enforcement.md - Trust and bundle integrity](continuous-enforcement.md#trust-and-bundle-integrity) for the full bundle trust model.

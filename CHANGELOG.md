# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).










## [0.6.5](https://github.com/rvben/verg/compare/v0.6.4...v0.6.5) - 2026-06-20

### Added

- **engine**: graceful Ctrl-C - finish in-flight hosts, skip the rest ([4b7c15a](https://github.com/rvben/verg/commit/4b7c15a9d8f21a72594715377ba42aaa3fde02f9))
- **resources**: add sensitive attribute and redact changelog payloads ([b133812](https://github.com/rvben/verg/commit/b133812ac63bd6b8934da743f3f911ad49d79aec))
- **engine**: per-host timeout so a hung host fails instead of blocking ([4936134](https://github.com/rvben/verg/commit/4936134c0404c0ab97b5926eb2b61456b537c2c3))
- **resources**: add write_atomic helper for managed config files ([eb6bc26](https://github.com/rvben/verg/commit/eb6bc266e0fc48b5fb02ca2fa1a9bc9a9d1a885c))
- **agent**: verify agent integrity locally and on the remote; atomic install ([3bcea2a](https://github.com/rvben/verg/commit/3bcea2a1a66965002152c9c20a435be246e1471b))
- **agent**: embed agent-binary checksum manifest at build time ([58acce5](https://github.com/rvben/verg/commit/58acce5eafdaae4e98bf6929d7b6d6c96308103a))
- **ssh**: enforce a uniform host-key policy (default StrictHostKeyChecking=yes) ([8f002c0](https://github.com/rvben/verg/commit/8f002c0958cece8a2d34636039e674d028c55b8a))
- **config**: reject unknown top-level state keys; add --lax-config ([5e76e6f](https://github.com/rvben/verg/commit/5e76e6f35994c99a1557b0080448c832a28e8384))
- **config**: validate resource types, props, and special-key types ([0c7da0f](https://github.com/rvben/verg/commit/0c7da0f01c9a1c4accc6d934e182bb6773ffa122))
- **config**: add ConfigPolicy and resource field registry ([9fae2e2](https://github.com/rvben/verg/commit/9fae2e2577555bf8818fd5821dfa4e6a0dcd503b))
- **resources**: add ScopedTempDir helper (unique 0700 temp dir) ([df04f63](https://github.com/rvben/verg/commit/df04f6399fb6da63e0ae910b1f8ad640cf343694))

### Fixed

- **config**: accept IPv6 host addresses; clarify changelog redaction policy ([a666c6d](https://github.com/rvben/verg/commit/a666c6db6feb96681f4037a4a2b7d1eb51ec5920))
- **config**: validate host fields; propagate state read errors; coerce vars scalars ([15145eb](https://github.com/rvben/verg/commit/15145eb856ba5d4ae8d1e6500fcb579656e3fbae))
- **cli**: structured errors on hot paths; confirmation maps to invalid_config ([dc1a9e2](https://github.com/rvben/verg/commit/dc1a9e21ec413639b7d74558c68e31d737384caf))
- **agent**: bound stdin read and pin a minimal PATH for commands ([e16a2b1](https://github.com/rvben/verg/commit/e16a2b1d5edd990d57345173c939cce3477bf731))
- **ssh**: apply connect + keepalive timeouts to all ssh/scp calls ([96ad97e](https://github.com/rvben/verg/commit/96ad97e2322f297946588fa5e0576ccce2288e76))
- **resources**: write managed config files atomically ([9cd0804](https://github.com/rvben/verg/commit/9cd0804c04128d849cd2a18eb7787053b8836612))
- **directory**: check owner and group drift independently ([c1693b0](https://github.com/rvben/verg/commit/c1693b045cd4b34a1221fe68dc185737aba5f797))
- **download**: error when chmod/chown fail instead of swallowing it ([c3737b7](https://github.com/rvben/verg/commit/c3737b79c7ab144d037c5b89653f34dacfea38e7))
- **sysctl**: error on failed read; persist by exact key match ([d8b6418](https://github.com/rvben/verg/commit/d8b641888d13f5fa29d9f9a98336c45b01b3dcb5))
- **service**: parse systemctl is-enabled output instead of exit code ([87a2021](https://github.com/rvben/verg/commit/87a202131e7c08550d0d0b726aec1a0f9d01fe8e))
- **pkg**: treat dpkg config-files state as not installed ([b1a0f23](https://github.com/rvben/verg/commit/b1a0f23da5100aa0e3c48e6197be645a37e2ed7d))
- **agent**: validate embedded checksum format before install; expose --skip-agent-checksum in schema ([c527d4b](https://github.com/rvben/verg/commit/c527d4b81a939b1cabb38b5997c5d7a67d0c77a1))
- **download**: use private per-run temp dirs and harden extraction ([2111c87](https://github.com/rvben/verg/commit/2111c8755f5f8587b1f8b64a57bc5b372c041556))
- **cron**: reject control characters in user, mailto, and env fields ([0cb6eda](https://github.com/rvben/verg/commit/0cb6edab29705cce5af33db3785993076178e4c9))
- **when**: treat missing fact in != comparison as skip ([114c16a](https://github.com/rvben/verg/commit/114c16a0fcfdc7ad33370d5419cbb4f171da0487))

### Performance

- **ssh**: gather facts once per host; bound agent output in errors ([9288156](https://github.com/rvben/verg/commit/92881565f4ae16722e1468afef11e048ff532395))

## [0.6.4](https://github.com/rvben/verg/compare/v0.6.3...v0.6.4) - 2026-06-11

### Added

- bring verg to clispec v0.2 compliance (24/24) ([9c38c62](https://github.com/rvben/verg/commit/9c38c62eca47a5fdf65bedfb1b7d81470ec32d9f))

## [0.6.3](https://github.com/rvben/verg/compare/v0.6.2...v0.6.3) - 2026-05-25

### Fixed

- **agent**: rewrite register interpolation loop as while-let ([38643c2](https://github.com/rvben/verg/commit/38643c2e66d85b89ec2a16aaaedcddb71e5c938c))
- **cmd**: tolerate broken pipe when child ignores stdin ([386e1ac](https://github.com/rvben/verg/commit/386e1ace6ac1d8496ab1afea6af319c51335f73f))

## [0.6.2](https://github.com/rvben/verg/compare/v0.6.1...v0.6.2) - 2026-04-03

## [0.6.1](https://github.com/rvben/verg/compare/v0.6.0...v0.6.1) - 2026-04-03

## [0.6.0](https://github.com/rvben/verg/compare/v0.5.0...v0.6.0) - 2026-03-29

### Added

- **`cron` resource type**: manages `/etc/cron.d/<name>` files declaratively. Supports single-job form (`schedule` + `command`) and multi-job form (`jobs` array). Validates name (path-safe characters only), schedule (5-field with range checks), and command (no newlines). Enforces `0644` permissions. Whole-file ownership is cleaner and more reliable than per-line cron management.

- **`stdin` on `cmd` resource**: pipe sensitive data to a command's stdin via `stdin = "{{ password }}\n{{ password }}\n"`. Content is treated as sensitive — never echoed in diffs, logs, or error messages. Uses a write thread to prevent deadlock when command output is large. Enables non-interactive use of tools like `smbpasswd`.

- **Inventory data in templates**: `inventory.hosts` (name → `{address, groups}`) and `inventory.groups` (group → `[names]`) are now available in all minijinja templates. Enables config generation driven by live inventory — e.g. Prometheus scrape targets that automatically include new hosts. Only `address` and `groups` are exposed; vars and credentials are never leaked.


## [0.5.0](https://github.com/rvben/verg/compare/v0.4.0...v0.5.0) - 2026-03-29

### Added

- **verg**: support bare service names and docker: legacy prefix in notify targets ([69ab1e2](https://github.com/rvben/verg/commit/69ab1e262e57bc67332870e4b47fdd74caac3cfb))
- **verg**: update schema with register, handler, notify, and template fields ([0abfe4d](https://github.com/rvben/verg/commit/0abfe4d078098fde0dc4f61fabe258899da157ee))
- **verg**: handler execution, register interpolation, and safe notify in agent ([487d15d](https://github.com/rvben/verg/commit/487d15d47cc887e9db891b4a2f8c1b72ee3463c4))
- **verg**: register sentinel pass-through and dependency validation ([e359613](https://github.com/rvben/verg/commit/e3596138e714b0a963dbb7b7a2f8bc8462dee7f3))
- **verg**: extract handler and register fields in bundle builder ([f894a58](https://github.com/rvben/verg/commit/f894a58b7e9e47dad6e4e0248211c30c4b1a5081))
- **verg**: add register stdout capture to cmd resource ([f91bfec](https://github.com/rvben/verg/commit/f91bfece1bee6e07780a079c9fc7f6fd14400318))
- **verg**: add handler, register, output fields to resource types ([4de1249](https://github.com/rvben/verg/commit/4de1249a551cee68bf061c4162820fbffc3f1aff))
- **verg**: add template opt-in for source file rendering ([1205fa4](https://github.com/rvben/verg/commit/1205fa48c388bcd8c0953be0d6dc9d94e1448363))
- **verg**: replace hand-rolled interpolator with minijinja template engine ([c3ec312](https://github.com/rvben/verg/commit/c3ec31218e0f8641040de3a11d46e2712c32b471))

### Fixed

- **verg**: validate docker paths, dry-run register messaging, template env_file ([b00260b](https://github.com/rvben/verg/commit/b00260b80d699b1eca9b13fb88aa1e833712a9d4))
- **verg**: use distinctive sentinel end marker to avoid collisions ([753bf9f](https://github.com/rvben/verg/commit/753bf9fbe48b5886d2c083d1bb62157cb6ec027d))
- **verg**: simplify render with render_str and restore missing tests ([21fdf18](https://github.com/rvben/verg/commit/21fdf18966037d4aa01136ddce824f46ced4f5c6))

### Performance

- **verg**: reuse minijinja environment across render calls ([5cc8602](https://github.com/rvben/verg/commit/5cc860287bbf8802bc0b678012fa6de32c0e301e))

## [0.4.0](https://github.com/rvben/verg/compare/v0.3.0...v0.4.0) - 2026-03-28

### Added

- **verg**: add system facts and when conditionals ([ccdfe0b](https://github.com/rvben/verg/commit/ccdfe0b89c328315506679ca2d002e64ad61b649))
- **verg**: add directory resource type with ownership and mode management ([e912a85](https://github.com/rvben/verg/commit/e912a85bf94bcb9ce44d90725797aa7cef368fad))
- **verg**: support docker: prefix in notify for compose restarts ([0bbf82b](https://github.com/rvben/verg/commit/0bbf82b0b56af2453bdc22e50faa15290d90a176))

### Fixed

- **e2e**: use SSH config alias and tighten test assertions ([dcf0664](https://github.com/rvben/verg/commit/dcf0664242a1aca16f3d44d4ee00aac5f7140698))
- **verg**: make download resource convergent and remove phantom template schema ([1e94f7a](https://github.com/rvben/verg/commit/1e94f7a6d7066316a53e730c680a7ee10fb7e41e))
- **verg**: unify exit code handling and reject --parallel 0 ([c81a853](https://github.com/rvben/verg/commit/c81a8535edc2bb54158b892e67d8d0acfe3457aa))
- **verg**: gracefully handle unreachable hosts instead of aborting ([8676a1c](https://github.com/rvben/verg/commit/8676a1c180a63e8b205a2c99023b290728a0a25a))
- **verg**: clean up old agent version cache after download ([4e1025e](https://github.com/rvben/verg/commit/4e1025e7c432016d38e0246e1e6e7c4fd7fefd89))
- **verg**: version agent binary cache to prevent stale binaries ([e6ce57d](https://github.com/rvben/verg/commit/e6ce57df155b0307483feb78c6d5a58c043a4484))

## [0.3.0](https://github.com/rvben/verg/compare/v0.2.1...v0.3.0) - 2026-03-28

### Added

- **verg**: add $env. secret references in variables ([c4d06c2](https://github.com/rvben/verg/commit/c4d06c2882a925c98567249cbb4960baa84f1269))
- **resources**: add download resource type for binary installs ([b71dc2a](https://github.com/rvben/verg/commit/b71dc2a526747af7608353ed8eb861a58987771e))
- **resources**: add docker_compose resource type ([75b3a50](https://github.com/rvben/verg/commit/75b3a504890c0a621a0fe2fdf03242807bc33e0a))
- **verg**: add apt_repo resource type ([0769c6c](https://github.com/rvben/verg/commit/0769c6cf560aded01f81daff72c5c8c1e901eb4f))

## [0.2.1](https://github.com/rvben/verg/compare/v0.2.0...v0.2.1) - 2026-03-28

### Fixed

- **verg**: build agent with musl for glibc-independent Linux binaries ([f9d2abd](https://github.com/rvben/verg/commit/f9d2abdf87543b3296f9398e8a25b2316df56e26))

## [0.2.0] - 2026-03-28

### Added

- **verg**: auto-download agent binaries from GitHub releases ([609ed1f](https://github.com/rvben/verg/commit/609ed1fdbf7d583f2cffa2c49186297042e5f62b))
- **verg**: add notify/restart, arch-aware agent push, apt cache update ([9d169b1](https://github.com/rvben/verg/commit/9d169b1d684e0660dda3afac96b090baba79c221))
- **inventory**: add SSH port support in host definitions ([60be433](https://github.com/rvben/verg/commit/60be4339a8ff762dd6f29a8d49849671a18023e2))
- **verg**: add e2e test infrastructure and SSH config support ([55d2583](https://github.com/rvben/verg/commit/55d25837488e1c013279807680c10639080c8ecd))
- **schema**: add schema command for agent introspection ([329ea6c](https://github.com/rvben/verg/commit/329ea6c1dfac187098251e7a6383991c309be228))
- **verg**: add structured change logging for apply runs ([1882752](https://github.com/rvben/verg/commit/1882752fed6c51da3fdb291d65d2c20ad861ba0c))
- **transport**: add SSH transport with binary push and version caching ([a50bfea](https://github.com/rvben/verg/commit/a50bfea6ea9e77c4b5b7466aaeb2e2265c6be3a7))
- **resources**: implement resource executors and agent binary ([d4adcd7](https://github.com/rvben/verg/commit/d4adcd7e4f8cdee85283951209b362d9e537b8a0))
- **bundle**: add bundle builder for host-specific task payloads ([64f12e5](https://github.com/rvben/verg/commit/64f12e5aaa662fa533c152d7cbd5ee898ce40e3f))
- **resources**: add resource types, result tracking, and DAG resolution ([c8f6935](https://github.com/rvben/verg/commit/c8f6935aa53224047e9385ce9c609e29d3b693a5))
- **state**: add state file parsing and resource declaration extraction ([174be86](https://github.com/rvben/verg/commit/174be86988d777f243bd18f5592e88d7c74a44fc))
- **inventory**: add target selector parsing and inventory filtering ([f88b3d6](https://github.com/rvben/verg/commit/f88b3d6b7b86896d3c6db4393608c3cb76ecc2e2))
- **state**: add variable interpolation engine ([4d47b36](https://github.com/rvben/verg/commit/4d47b36b3f7fc221f3af17b6996aab83414c18c0))
- **inventory**: add inventory system with static hosts and group variables ([7fc2e4b](https://github.com/rvben/verg/commit/7fc2e4b93fd4cdf3903a56407ecebcb46c91918d))
- **verg**: scaffold project with error types and output config ([5dcb989](https://github.com/rvben/verg/commit/5dcb989a7b328483f8f8b1ef887b0769e61d4c5d))

### Fixed

- **verg**: resolve source files on control machine, not target ([565b4a7](https://github.com/rvben/verg/commit/565b4a78aaf8a225101e669b5ddec0644546e20e))
- **verg**: shorten keyword for crates.io publishing ([9e96bec](https://github.com/rvben/verg/commit/9e96bec81f86873af312d7a102935d7ce875b52c))
- **verg**: use terminal-aware colors via owo-colors ([3d64bf0](https://github.com/rvben/verg/commit/3d64bf02a173d04a3b8d0fdbf10d6767b20e745b))
- **verg**: wire changelog into apply, fix owner check and path consistency ([bec4562](https://github.com/rvben/verg/commit/bec456277fb16384cc32d02e4d7112f0e134997a))
- **verg**: collapse nested if per clippy ([a146824](https://github.com/rvben/verg/commit/a146824d2efa6f3fd499fea1dbb08998e63503a5))

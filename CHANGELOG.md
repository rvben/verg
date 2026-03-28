# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).




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

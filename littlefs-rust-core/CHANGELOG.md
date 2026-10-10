# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/PhotomancerArt/littlefs-rust/compare/littlefs-rust-core-v0.1.0...littlefs-rust-core-v0.1.1) - 2026-10-10

### Fixed

- *(littlefs-rust-core)* write the FCRC tag in lfs_dir_commitcrc
- *(littlefs-rust-core)* size loop_limits tag-walk caps from the block
- *(littlefs-rust-core)* wrap unsigned tag and alignment arithmetic as C does

### Other

- *(ci)* fix clippy 1.97 lints and cargo-deny shlex duplicate
- release v0.1.0

## [0.1.0](https://github.com/light-player/littlefs-rust/releases/tag/littlefs-rust-core-v0.1.0) - 2026-03-10

### Other

- *(release)* enable crates.io publishing
- format
- rename project to littlefs-rust

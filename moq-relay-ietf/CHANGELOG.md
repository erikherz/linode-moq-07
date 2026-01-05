# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.5](https://github.com/erikherz/linode-moq-07/compare/moq-relay-ietf-v0.7.4...moq-relay-ietf-v0.7.5) - 2026-01-05

### Added

- Add --devs flag for HTTPS dev server
- Make moq-transport Session generic over transport trait
- Add WebSocket-based WebTransport support for Safari compatibility

### Fixed

- Change dev server to HTTP on port 80 to avoid WebSocket conflict

### Other

- Add explicit runtime shutdown with timeout for WebSocket sessions
- Run WebSocket sessions on dedicated thread with 64MB stack
- Use task spawning and abort handles for stack-safe cleanup
- Spawn session components as separate tasks to prevent stack overflow
- Add recursion guards to Drop implementations
- Spawn waker notifications to break drop chains
- Increase worker thread stack to 64MB for debugging
- Use Box::pin for async futures to reduce stack usage
- Replace all FuturesUnordered with tokio::spawn in relay
- Replace FuturesUnordered with select! in relay session wrapper
- Increase tokio worker thread stack size to 8MB
- Replace FuturesUnordered with tokio::spawn in relay main loop
- Fix WebSocket stream corruption by calling finish() instead of reset
- Spawn upstream subscribe in background task
- Add upstream subscribe support for cross-relay stream discovery

## [0.7.4](https://github.com/englishm/moq-rs/compare/moq-relay-ietf-v0.7.3...moq-relay-ietf-v0.7.4) - 2025-02-24

### Other

- updated the following local packages: moq-transport

## [0.7.3](https://github.com/englishm/moq-rs/compare/moq-relay-ietf-v0.7.2...moq-relay-ietf-v0.7.3) - 2025-01-16

### Other

- cargo fmt
- Change type of namespace to tuple

## [0.7.2](https://github.com/englishm/moq-rs/compare/moq-relay-ietf-v0.7.1...moq-relay-ietf-v0.7.2) - 2024-10-31

### Other

- updated the following local packages: moq-transport

## [0.7.1](https://github.com/englishm/moq-rs/compare/moq-relay-ietf-v0.7.0...moq-relay-ietf-v0.7.1) - 2024-10-31

### Other

- release

## [0.7.0](https://github.com/englishm/moq-rs/releases/tag/moq-relay-ietf-v0.7.0) - 2024-10-23

### Other

- Update repository URLs for all crates
- Rename crate

## [0.6.1](https://github.com/kixelated/moq-rs/compare/moq-relay-v0.6.0...moq-relay-v0.6.1) - 2024-10-01

### Other

- update Cargo.lock dependencies

## [0.5.1](https://github.com/kixelated/moq-rs/compare/moq-relay-v0.5.0...moq-relay-v0.5.1) - 2024-07-24

### Other
- update Cargo.lock dependencies

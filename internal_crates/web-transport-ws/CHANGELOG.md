# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/erikherz/linode-moq-07/compare/web-transport-ws-v0.2.0...web-transport-ws-v0.2.1) - 2026-01-05

### Added

- Add WebSocket-based WebTransport support for Safari compatibility

### Other

- Add global drop depth tracking to debug stack overflow
- Fix SendStream Drop to avoid spawning tasks
- Fix stream corruption by not sending STOP_SENDING in RecvStream Drop
- Fix stack overflow crash by calling finish() in SendStream Drop
- Add WebSocket frame logging to debug stream corruption

## [0.1.4](https://github.com/moq-dev/web-transport/compare/web-transport-ws-v0.1.3...web-transport-ws-v0.1.4) - 2025-11-14

### Other

- Avoid some spurious semver changes and bump the rest ([#121](https://github.com/moq-dev/web-transport/pull/121))
- Initial web-transport-quiche support ([#118](https://github.com/moq-dev/web-transport/pull/118))

## [0.1.3](https://github.com/moq-dev/web-transport/compare/web-transport-ws-v0.1.2...web-transport-ws-v0.1.3) - 2025-10-25

### Other

- Don't use a newer Rust method. ([#115](https://github.com/moq-dev/web-transport/pull/115))

## [0.1.2](https://github.com/moq-dev/web-transport/compare/web-transport-ws-v0.1.1...web-transport-ws-v0.1.2) - 2025-10-17

### Other

- Change web-transport-trait::Session::closed() to return a Result ([#110](https://github.com/moq-dev/web-transport/pull/110))
- Use workspace dependencies. ([#108](https://github.com/moq-dev/web-transport/pull/108))

## [0.1.1](https://github.com/moq-dev/web-transport/compare/web-transport-ws-v0.1.0...web-transport-ws-v0.1.1) - 2025-09-04

### Other

- Publish NPM package. ([#96](https://github.com/moq-dev/web-transport/pull/96))

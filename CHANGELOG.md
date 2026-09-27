# Changelog

## [0.4.0](https://github.com/butlerx/metrics_ipc_collector/compare/0.3.0...v0.4.0) (2026-09-27)

### Features

* add pipe transport support ([bd4c3b4](https://github.com/butlerx/metrics_ipc_collector/commit/bd4c3b489dfebb1e065ef802aab6ade12d278f17))

### Bug Fixes

* **ci:** preserve action version annotations ([d150846](https://github.com/butlerx/metrics_ipc_collector/commit/d150846d7071085ab93c474783d5907a40609def))
* finish pipe support integration ([62e3914](https://github.com/butlerx/metrics_ipc_collector/commit/62e39146f83d2c55520e462c27732007e0932f43))
* preserve socket API and runtime errors ([01dacae](https://github.com/butlerx/metrics_ipc_collector/commit/01dacaebe2869baa4b2b23e0cef09c7235203a33))

## [0.3.0](https://github.com/butlerx/metrics_ipc_collector/compare/0.2.2...0.3.0) (2025-11-02)

### Features

* add support for using a Tokio task instead of a thread for collecting metrics ([334684d](https://github.com/butlerx/metrics_ipc_collector/commit/334684d6aea6cb9884a3719838f95d29c1d278cb))

### Code Improvements

* clean up event serialization and deserialization logic ([43079c2](https://github.com/butlerx/metrics_ipc_collector/commit/43079c24ce3552081efab1525af0871eb7411e89))

### Documentation

* improve Rust documentation ([6573847](https://github.com/butlerx/metrics_ipc_collector/commit/65738474f5061f254c320ea165016f916d2009ae))

## [0.2.2](https://github.com/butlerx/metrics_ipc_collector/compare/0.2.1...0.2.2) (2025-08-12)

### Bug Fixes

* log failed deserialization at trace level ([5aa2117](https://github.com/butlerx/metrics_ipc_collector/commit/5aa2117968804f7b9f63a966951e3bef19a04f82))

## [0.2.1](https://github.com/butlerx/metrics_ipc_collector/compare/0.2.0...0.2.1) (2025-08-12)

### Bug Fixes

* remove an unnecessary log line ([68b504f](https://github.com/butlerx/metrics_ipc_collector/commit/68b504fa4a24b499c74c223efcb22f0a974923b2))

## [0.2.0](https://github.com/butlerx/metrics_ipc_collector/compare/0.1.0...0.2.0) (2025-08-12)

### Features

* use MessagePack for more efficient socket communication ([cf122a9](https://github.com/butlerx/metrics_ipc_collector/commit/cf122a91d45ef92490a1b57d22b86a74031eba7f))

## [0.1.0](https://github.com/butlerx/metrics_ipc_collector/releases/tag/0.1.0) (2025-08-12)

### Features

* initial release ([012467b](https://github.com/butlerx/metrics_ipc_collector/commit/012467b93b56cbe909716586903942f4cd181208))

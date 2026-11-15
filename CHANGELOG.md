# Changelog

## [0.5.1](https://github.com/butlerx/metrics_ipc_collector/compare/v0.5.0...v0.5.1) (2026-09-29)


### Bug Fixes

* send counters as increments and arbitrate replaced senders ([4a05612](https://github.com/butlerx/metrics_ipc_collector/commit/4a0561258fde0dd683743dddb84154abdc5cffff))

## [0.5.0](https://github.com/butlerx/metrics_ipc_collector/compare/v0.4.1...v0.5.0) (2026-09-29)


### ⚠ BREAKING CHANGES

* frames now carry a magic and version header. The deprecated `IPCCollector`, `IPCRecorder` and `IPCRecorderBuilder` aliases were removed. `interprocess` 2.4 or later is required.
* the wire format changed, so 0.5 senders and collectors cannot talk to 0.4. `start_collecting` returns `CollectorHandle` instead of `()`. `PipeSender` is now the blocking pipe type under the `tokio` feature too. `MetricsError::PipeCollectorConsumed` was removed and `FrameTooLarge` / `CollectorPanicked` were added.

### Features

* harden IPC framing and extend collector and recorder APIs ([fdb5dc2](https://github.com/butlerx/metrics_ipc_collector/commit/fdb5dc2b57ec47421cca297f2e25abd23db99a63))
* reconnect, sender labels, handle caching and internal metrics ([686f84b](https://github.com/butlerx/metrics_ipc_collector/commit/686f84b07c9ad5f880a8e3823614a28931e6b468))
* version the wire format and make socket addresses explicit ([9cfecb5](https://github.com/butlerx/metrics_ipc_collector/commit/9cfecb5a8fa9d6cc1771174ae55d3183ff2108e3))


### Bug Fixes

* separate recorder counters per process and stop sockets promptly ([017a6c3](https://github.com/butlerx/metrics_ipc_collector/commit/017a6c3e09296ad48ff8922b87b4bfb435fcac29))

## [0.4.1](https://github.com/butlerx/metrics_ipc_collector/compare/v0.4.0...v0.4.1) (2026-09-27)


### Bug Fixes

* **ci:** publish without a Cargo lockfile ([63f74a3](https://github.com/butlerx/metrics_ipc_collector/commit/63f74a3a11216a263a51bd7ac16817a2d46b4e25))


### Documentation

* backfill changelog ([8debf4f](https://github.com/butlerx/metrics_ipc_collector/commit/8debf4f2232731ba73e2f5df955aa200ecad4e2d))

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

# Validation

Validated on 2026-10-09:

- `cargo test --locked`: four unit tests and five loopback integration tests pass.
  The integration suite covers TCP and WebSocket discovery, multiple ROS domains,
  subscriptions, retained samples, disconnects, and runtime type reflection.
- The reflection test publishes a message type absent from the compiled registry,
  serves it through REP-2016 `GetTypeDescription`, verifies the advertised RIHS01
  hash, and decodes string and numeric fields from the received CDR sample.
- `cargo clippy --all-targets --locked -- -D warnings` passes.
- `cargo fmt --all -- --check` passes.
- The locked optimized `wasm32-unknown-unknown` build passes from `wasm/`.

The build reports two known dependency/toolchain warnings: HIRoZ codegen skips
`example_interfaces/WString` because wide strings are unsupported, and the WASM
nightly warns that the `atomics` target feature is unstable. Cargo also reports
an upstream `static_init_macro` future-compatibility notice on native builds.

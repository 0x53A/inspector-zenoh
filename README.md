# Inspector / Zenoh

A native desktop and Rust/WASM browser app for inspecting ROS 2 traffic directly
through a Zenoh router. It learns message definitions at runtime through ROS 2
type-description services and includes standard definitions as a fast fallback.

## Native desktop

```sh
cargo run --locked --bin inspector-zenoh
```

The desktop app uses the same topic discovery, schemas and inspection views as
the browser app. Enter `tcp/rover-host:7447` (the default is
`tcp/127.0.0.1:7447`) or `ws/rover-host:7448` and click Connect & discover.
It connects directly to the router and needs no web server or browser.
Linux builds support both Wayland and X11 and use OpenGL for rendering.
On NixOS, use the included shell to expose the graphics libraries:

```sh
nix-shell --run 'cargo run --locked --bin inspector-zenoh'
```

With direnv installed, run `direnv allow` once in this directory. The included
`.envrc` loads `shell.nix` automatically; then use `cargo run --locked`.

For a standalone optimized executable, run `cargo build --release --locked --bin inspector-zenoh`; the executable is `target/release/inspector-zenoh`.

To try either app locally, run `cargo run --locked --example local_fixture`
in another terminal. Use `tcp/127.0.0.1:17447` on desktop or
`ws/127.0.0.1:17448` in either app. The fixture advertises topics in domains
0, 17 and 42.

## Startup and domain discovery

```sh
cargo run --locked -- --endpoint tcp/rover-host:7447
# Optional initial display filter:
cargo run --locked -- --endpoint=tcp/rover-host:7447 --domain-id 123
```

`--endpoint` connects at startup; omitting it
starts disconnected. `--help` lists the options. `--domain` is also accepted as
an alias for `--domain-id`.

Discovery listens to `@ros2_lv/**`, including existing and newly advertised
nodes and endpoints across all domains visible through the connected router.
The sidebar shows live domain IDs, including domains with no topics yet.
Select a domain to filter, or **All domains** to view everything, without
reconnecting. Identical topic names in different domains remain separate.
Selecting a topic announces a subscriber in that topic's domain; the inspector's
own announcements do not appear in the discovery list.

Only domains advertised through this router can be discovered. This does not
scan other routers or DDS networks, and domains disappear when their last
external node/endpoint disappears.

## Browser build and run

The `main` branch is deployed to
<https://0x53a.github.io/inspector-zenoh/> by GitHub Actions. The first visit may
reload once while a service worker enables the isolation headers required by
threaded WASM.

```sh
./build-web.sh
python3 web/serve.py 8090
```

Open <http://localhost:8090>, enter `ws/rover-host:7448`, then connect.
`?endpoint=ws/rover-host:7448` auto-connects; optional `&domain=0` sets the initial
domain filter.
The app only subscribes to a topic when selected. Selecting another topic drops
the previous subscription. Disconnect closes the session.

The browser uses the native Zenoh protocol over WebSockets through Zenoh/HIRoZ.
The router needs an ordinary Zenoh WebSocket listener,
for example `listen/endpoints: ["tcp/0.0.0.0:7447", "ws/0.0.0.0:7448"]`.
The Remote API plugin's WebSocket port is a different protocol and is not usable
here. No inspector backend is required; `serve.py` serves static files only.

The threaded WASM build needs nightly Rust with `rust-src`, plus wasm-bindgen-cli
matching the locked wasm-bindgen version. `wasm/.cargo/config.toml` rebuilds std
with atomics. Browser execution needs a secure context and cross-origin isolation;
the supplied localhost server sets COOP/COEP. Remote hosting should use HTTPS and
a browser-compatible secure WebSocket endpoint.

## Views

- Live domain discovery, topic search, publisher/subscriber counts and node names.
- A session-stable, virtualized topic list; disappeared topics remain in place as
  gray offline entries, while the footer distinguishes live and seen counts.
- Colored domain filters, topic cards, and live telemetry counters.
- Latest message with received sample/byte counters, one-second rate measurements,
  and a pause control that freezes the displayed sample while reception continues.
- Overview overrides for Twist and PointCloud2.
- Generic expandable fields, nested messages, arrays and byte previews.
- Raw payload and schema/hash information. Types absent from the embedded registry
  are fetched from a publisher's REP-2016 `GetTypeDescription` service. If no
  publisher can supply one, the payload remains inspectable as raw bytes.

The latest payload is coalesced in the transport layer and decoded at most ten
times per second. Counts include all received samples, even ones never displayed.
Payloads above 8 MiB are counted but not retained. There is no history or recording
yet. Hex previews display the first 1024 bytes; smaller-than-limit payloads remain
complete for decoding. The current HIRoZ dynamic decoder handles CDR little-endian;
unsupported encodings are reported as decode errors.
Decoded trees are limited to 100,000 values and 64 nesting levels. A validation
pass checks sequence sizes before allocation; messages exceeding this budget
remain available in Raw.

## Message definitions

The generic inspector embeds the standard interfaces supplied by HIRoZ for its
selected ROS distribution. It contains no message-source submodules. Separately
pinned packages and deployment-specific interfaces belong in a specialized
wrapper and are injected through its registry.

`build.rs` generates bindings and the built-in registration table, including
action-feedback topic envelopes, from HIRoZ's bundled definitions.

Embedded RIHS01 hashes must match publishers. On an unknown type or hash mismatch,
the inspector queries a publishing node for its runtime type description, rebuilds
nested schemas, verifies the publisher-reported hash, and caches the result for the
connection. Nodes made with current rclcpp/rclpy expose this service by default;
plain HIRoZ nodes need `with_type_description_service()`. Failure or timeout does
not interrupt the topic subscription, and the Raw view remains available. The code
generator still skips unsupported `wstring` definitions.

Audit all currently advertised topic types and hashes without subscribing to
message payloads or publishing commands:

```sh
cargo run --locked --example audit_schemas -- \
  --endpoint tcp/robot:7447
```

The audit exits nonzero for missing/mismatched schemas or no discovered topics.
Use `--domain-id 123` to audit one domain, and `--seconds 15` to allow more time
for discovery.

## Deployment-specific message types

Applications can add generated HIRoZ schemas without forking the inspector. A
specialized binary builds a `Registry`, registers its private message types, and
passes it to the library:

```rust
fn main() -> eframe::Result {
    let mut types = inspector_zenoh::schema::Registry::new();
    my_robot_msgs::register_all(&mut types);
    inspector_zenoh::run_with_custom_types(types)
}
```

Use `Registry::register::<T>()` for ordinary messages and
`Registry::register_action_feedback::<T>()` for action feedback envelopes.
Custom definitions replace built-in entries with the same advertised ROS type
name. Browser wrappers can call `start_with_custom_types` in the same way.

## Structure and provenance

- `discovery.rs`: ROS liveliness graph, parsed using HIRoZ's protocol implementation;
  topics retain the full data key and hash, and removals update endpoint counts.
- `transport.rs`: selected-topic subscription, bounded latest-sample state, and
  per-domain REP-2016 clients. ROS subscription liveliness is announced for lazy publishers.
- `schema.rs`: generated registry and hash-checked dynamic CDR decoding.
- `main.rs`: native desktop entry point.
- `app.rs` / `views.rs`: shared desktop/browser UI and renderer overrides, separate from transport.
- `wasm/`: browser entry point and Zenoh fork patches, isolated from native tests.

Patterns informed by HIRoZ Union (`hu`)'s graph/schema tooling. This app does
not load `hu` plugins or require its
native HTTP server. Initially targets HIRoZ / `rmw_zenoh_cpp`, not DDS bridge admin
discovery or service traffic. It provides no publish/service-call controls.

```sh
cargo test --locked

# Optional local router and three sample topics; no rover connection:
cargo run --example local_fixture --locked
# Browser endpoint: ws/127.0.0.1:17448
```

Both Cargo lockfiles pin the fork revisions used by this app. The surrounding
ROS/Zenoh ecosystem and WASM forks are still evolving; changes should be validated
against a local router and known typed publisher before rover deployment.

## First-sample latency

Selecting a transient-local topic requests the latest retained sample from each
publisher, with a one-second history query timeout. Volatile topics wait for their
next publication. If a transient-local publisher is discovered after selection,
the subscription automatically enables history without requiring reselection.
The UI distinguishes subscription setup, waiting for data, and
no advertised publisher. Rapid topic selections skip queued intermediate choices.

To measure selection latency against the rover without publishing commands:

```sh
cargo run --locked --example first_sample -- \
  --endpoint tcp/robot:7447 --domain-id 123 \
  --topic /robot_description --topic /res/status --timeout 10
```

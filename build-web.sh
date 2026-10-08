#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
wasm_bindgen_bin="${WASM_BINDGEN:-}"
if [[ -z "$wasm_bindgen_bin" ]]; then
  wasm_bindgen_bin="$(command -v wasm-bindgen || true)"
fi
if [[ -z "$wasm_bindgen_bin" && -x "${CARGO_HOME:-$HOME/.cargo}/bin/wasm-bindgen" ]]; then
  wasm_bindgen_bin="${CARGO_HOME:-$HOME/.cargo}/bin/wasm-bindgen"
fi
if [[ -z "$wasm_bindgen_bin" ]]; then
  echo 'Install wasm-bindgen-cli matching wasm/Cargo.lock, or set WASM_BINDGEN.' >&2
  exit 1
fi
(
  cd wasm
  cargo build --target wasm32-unknown-unknown --release --locked
)
"$wasm_bindgen_bin" --target no-modules --out-dir web/pkg \
  "${CARGO_TARGET_DIR:-wasm/target}/wasm32-unknown-unknown/release/inspector_zenoh_wasm.wasm"
echo 'Built. Start with: python3 web/serve.py 8090'

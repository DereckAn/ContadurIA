#!/usr/bin/env bash
# Compila la e.firma a WebAssembly en static/efirma/ (requiere: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli --version 0.2.130)
set -euo pipefail
cd "$(dirname "$0")"
cargo build -p efirma --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir static/efirma target/wasm32-unknown-unknown/release/efirma.wasm

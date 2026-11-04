#!/usr/bin/env bash
# Refresh the crate licences built into Fogu (`fogu --licenses`) when Cargo.lock changes, for
# local builds (release CI regenerates them itself). Needs cargo-about (`cargo install --locked --features cli
# cargo-about@0.9.2`).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo about generate --locked -m crates/app/Cargo.toml about.hbs -o target/notices.txt
gzip -9 -n -c target/notices.txt > assets/updater/notices.gz

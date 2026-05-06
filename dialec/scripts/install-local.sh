#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/.." && pwd)"

cd "$repo_dir"
cargo install --path .

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
echo "installed dialec to $cargo_bin/dialec"

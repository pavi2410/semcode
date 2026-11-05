#!/usr/bin/env bash
# Reproduce the spike. Requires: uv, node, cargo.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CORPUS=/home/pavitra/Playground/semcode/corpus/httpx
W=/tmp/httpx-base
rm -rf "$W" && cp -a "$CORPUS" "$W" && cp "$HERE/fixture_semcode.py" "$W/"
git -C "$W" add -A && git -C "$W" -c user.email=x@x -c user.name=x commit -qm fixture
(cd "$W" && uv venv -q .venv && VIRTUAL_ENV=.venv uv pip install -q -e . pytest)
command -v ty >/dev/null || uv tool install ty@0.0.87
[ -d "$HERE/node/node_modules/basedpyright" ] || (cd "$HERE/node" && npm install basedpyright@1.40.2)
python3 -I "$HERE/lsp_bench.py" ty "$W" "$HERE/results/ty.json" -- ty server
WDP=1 python3 -I "$HERE/lsp_bench.py" basedpyright "$W" "$HERE/results/basedpyright.json" -- \
  node "$HERE/node/node_modules/basedpyright/langserver.index.js" --stdio
(cd "$HERE/ty-lib" && cargo build --release && ./target/release/ty-lib-spike "$W")

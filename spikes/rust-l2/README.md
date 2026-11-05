# rust-l2 spike: semantic backend for Rust (ra_ap_* library vs rust-analyzer over LSP)

Both harnesses run the same queries against ripgrep:
- find references to `grep_matcher::Matcher` and to `Searcher::search_path`
- go to definition through `use ...ContextSeparator as Separator`
- type of a local and of an expression range (hover)
- incoming calls of `Searcher::search_path`
- rename `Matcher::find_at` (a trait method with impls in other crates)
- an in-memory edit that adds a new caller, then the queries run again (incremental)

Always point the harnesses at a **copy** of the corpus. Build scripts write `target/`, and `--apply-rename` rewrites files.

    cp -r ../../corpus/ripgrep /tmp/rg-spike

## A: embedded library (`ra-lib/`)

The crates are pinned to `ra_ap_* =0.0.358`, which needs rustc >= 1.98. Workspace loading copies
rust-analyzer's `cli/scip.rs` and `cli/analysis_stats.rs` at git sha 2ad81e72.

    cd ra-lib && cargo build --release        # clean release build: 4m52s wall / 19.5 CPU-min (loaded 8-core box), 223 crates; leaf rebuild 3.5s
    ./target/release/ra-lib /tmp/rg-spike [--all-features] [--apply-rename]

## B: rust-analyzer over LSP stdio (`lsp-client/`)

This is a hand-rolled JSON-RPC client using only the Python stdlib.

`~/.cargo/bin/rust-analyzer` is a broken rustup proxy: the component isn't installed and it recurses into the mise shim. Use a real binary instead, for example Zed's:

    RA=~/.local/share/zed/languages/rust-analyzer/rust-analyzer-2026-10-12   # 0.3.3081
    python3 -I lsp-client/ra_lsp_bench.py $RA /tmp/rg-spike [utf-8|utf-16] [--apply-rename] [--all-features] [--no-diagnostics] [--no-open] [--disk-edit [--server-watcher]]

The cold load ends once `experimental/serverStatus` reports quiescent and every `$/progress` token has ended, including the Indexing (cache priming) token.

SCIP data point: `$RA scip /tmp/rg-spike --output /tmp/index.scip` took about 16 s, used about 1.0 GB peak RSS and wrote an 8.3 MB index.

To check a rename, run with `--apply-rename` on a copy, then run `cargo check --all-targets` in that copy. It passed for both A and B.

## Results on ripgrep, 2026-10-14

The box is a shared 8-core machine with a load average of 10-19, so treat timings as rough.

| | A ra_ap 0.0.358 | B LSP 0.3.3081 |
|---|---|---|
| cold load, no target/ | 14.0-15.3 s | 15.5 s |
| load with build scripts cached | 6.0-9.6 s | 6.4-11.4 s |
| RSS after load (including proc-macro-srv) | 900 MB | 935 MB |
| first refs `Matcher` / `search_path` | 6 / 70 ms | 185 / 820 ms |
| warm refs | 7 ms | 4 ms |
| goto through `as` alias | 11 ms, correct | 12 ms, correct |
| type of an expression range | 0.2 ms | 0.8 ms |
| incoming calls | 0.7 ms | 1 ms |
| rename `find_at` | 40-85 ms, 33 edits in 5 files and 4 crates; `cargo check` passed | 63-155 ms, same edits; `cargo check` passed |
| incremental edit, then incoming calls | 33 ms apply plus 30 ms, new caller seen | 20-80 ms, new caller seen, including an on-disk edit |
| `rust-analyzer scip` | 16 s, 1.0 GB, 8.3 MB index | |

The edit API changed across versions. From 0.0.310 to 0.0.358 (10 months), all 7 config structs we use changed. From 0.0.330 to 0.0.358, 1 of 7 changed, and from 0.0.350 to 0.0.358, none did.

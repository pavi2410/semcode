# Spike: Python semantic backend for semcode (2026-10-14)

Question: which backend should semcode use for Python name resolution, types, references, and rename, given that the host is written in Rust?

## Setup

- Target: a copy of `corpus/httpx` at `/tmp/httpx-base` (about 17.7k LOC), with a `.venv` (`uv pip install -e .`) and `fixture_semcode.py` added.
- The fixture covers:
  - `import httpx as h`
  - `from httpx import Client as C`
  - a parameter annotated `httpx.Client`
  - `with httpx.Client() as client`
  - a `MyClient(httpx.Client)` subclass that overrides `send` and calls `super().send`
  - a decoy `AsyncClient.send`
  - a line containing `"é😀"` to test position encodings
- Ground truth: 11 true references to `Client.send` (listed in `lsp_bench.py`). They are `self.send` x2, tests x4, and fixture x5. The decoys are `AsyncClient.send`, generator `flow.send`, the asgi `send` and docstrings.
- `lsp_bench.py` is a generic LSP driver. It answers `workspace/configuration`, measures process-tree peak RSS, and applies rename edits to a git clone so they can be diffed.
- `ty-lib/` is a Rust crate that uses ty as a library. It depends on `ty_ide`, `ty_project`, `ruff_db` and related crates, pinned by git rev `1ae6552a` (the ruff commit used by ty 0.0.87).
- `setup_and_run.sh` reproduces everything. Raw results are in `results/`.

## Measurements (ty 0.0.87, basedpyright 1.40.2, 8 cores)

The machine was shared and noisy: load average rose to 20 during the later runs. Ranges below cover 3 to 5 runs.

| | A1: ty as Rust library | A2: `ty server` (LSP) | B: basedpyright LSP |
|---|---|---|---|
| Setup | git dep on 7 crates; first build about 11 min, 216 crates, 39 MB binary, 732 MB `target/`; compiled first try | `uv tool install ty` (single static binary) | node + `npm i basedpyright` |
| Init / cold, until correct refs | full `check()` of the project in 1.1 s | first complete refs 0.33–1.1 s after spawn (lazy, no wait needed) | 5.8–6.5 s after spawn (refs must wait for the workDoneProgress `end`) |
| Peak RSS | 97 MB (after full check) | 83 MB | 350 MB (node) |
| Refs to `Client.send` | 11/11, 0 false positives; 13–17 ms (after the full check, so not a cold number) | 11/11 cold and warm; warm 2–5 ms | 11/11 once ready. **7/11 if queried before the "Found N source files" enumeration**, with no error. Warm 0.4–1.0 s |
| Alias-proof (`h.Client`, `C()`) | yes | yes | yes |
| Rename base `send` | 11 edits; **misses `super().send`**, which leaves broken code | same as A1 | 12 edits; correct, including `super().send` |
| Rename override (`MyClient.send`) | renames the subclass only (2 edits) | same | same |
| Override hierarchy propagation on rename | no | no | no |
| Incoming calls | 10/10 callers, 58 ms | 10/10, 25–60 ms | 10/10, 0.4–0.8 s |
| Hover (`with ... as client`, `r = client.send()`) | `Client`, `Response` | `Client`, `Response` | `client: Client`, `r: Response` |
| Incremental, new file + edit on disk | `apply_changes` 1 ms, re-query 69 ms, sees both | didChange 20 ms; on-disk change 14–30 ms. **Only after the client sends `didChangeWatchedFiles`**: ty registers for it and has no watcher of its own, so with no notification it sees nothing even after 4 s | didChange 0.2 s; on-disk change after didChangeWatchedFiles 1–2 s |
| Position encoding | byte `TextSize` offsets (convert yourself) | honours the client preference: utf-8, utf-16 or utf-32 (each verified on an emoji line) | **utf-16 only**; the utf-8 offer is ignored |
| Other APIs | goto def/decl/type/impl, type hierarchy, workspace symbols, semantic tokens, signature help, outgoing calls | same set over LSP | similar, but no type hierarchy |

### C: home-grown resolver on tree-sitter-python (estimate only)

- Name-only matching of `.send(` gives 36 code call sites, of which 11 are true. That is about 31% precision.
- A scope-graph resolver could also handle `self`, constructor-assigned locals, annotated parameters, aliases and `super()`. That gets roughly 6–8 of the 11.
- `with Client() as c` needs the return types of `__enter__` and `Self`. Separating `Client` from `AsyncClient` and `flow.send` needs real type inference.
- GitHub's stack-graphs repo is archived (`archived: true`, last push in Sep 2025).
- Verdict: weeks of work to reach about 70% recall, so not worth building. A tree-sitter layer is still useful as a fast L1 fallback.

## Gotchas

- **ty rename** leaves `super().send` unrenamed even though `find_references` returns it. I confirmed this with `ty check` on the renamed copy: it reports `unresolved-attribute`. Upstream bug.
- **No backend propagates rename across overrides.** semcode has to do that itself, using `goto_implementation` and type hierarchy.
- **basedpyright cold references are silently incomplete** until workspace enumeration finishes. To avoid this, enable `window.workDoneProgress`, answer `window/workDoneProgress/create`, and wait for `$/progress end` before querying. You also need to answer `workspace/configuration` to set `pythonPath`.
- **ty server will not notice on-disk edits** unless the host forwards file-watch events. semcode must watch files itself, either through `didChangeWatchedFiles` or by calling `apply_changes` in library mode.
- **Library mode, concurrency:** `apply_changes` takes `&mut db`, so concurrent queries need the snapshot/clone pattern that `ty_server` uses. Both ty and rust-analyzer use salsa. If semcode embeds `ra_ap_*` crates too, run `cargo tree -d -i salsa` and check for version conflicts.
- **ty crates are unstable as a library:** `publish = false`, version 0.0.0, no semver, and the API changes weekly. Pin a rev and expect breakage on every bump. The build cost and Salsa db threading (`&dyn Db`, `ProgramFile`) are moderate.
- ty is still beta; its type inference has gaps on dynamic code (pyright's is more mature).

## Recommendation

**Use ty, embedded as a library** (`ty_ide` + `ty_project`), pinned to a ty release tag.

- It is the most precise backend measured and uses about 4x less memory than basedpyright.
- Compared with basedpyright, cold-to-correct is about 5–10x faster, warm references about 100x, and incoming calls about 10x.
- It is native Rust with byte offsets, so semcode needs no extra process, no node, and no UTF-16 conversion.
- File changes go straight into the Salsa db.
- Wrap it behind a semcode-owned trait so we can fall back to `ty server` over LSP (same results, stable protocol) if tracking the API becomes too costly.
- Before using rename, add a post-rename verification step and hierarchy-aware expansion.

**What would reverse it:**
1. Upstream API churn costs more than about 1 day per bump. Then switch to `ty server` over LSP.
2. On real-world dynamic codebases (Django-style metaprogramming), ty's references or inference turn out materially worse than pyright's. Then use basedpyright for Python and accept node, 350 MB and UTF-16.
3. Astral changes the license or abandons ty.

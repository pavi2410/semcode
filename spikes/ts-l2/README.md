# Spike: TypeScript semantic backend for semcode (ts-l2)

Date: 2026-10-14. Target: zod (`packages/zod`, 333 .ts files, project of 595 files including libs).
The target was copied to `/tmp/zodspike`. `vitest`, `recheck`, and `@types/node` are symlinked into its
`node_modules` so the type check is clean. Without them, `types: ["vitest","recheck"]` gives TS2688 and
tsc skips semantic checking. Baseline was `tsc --noEmit -p packages/zod`: 28 pre-existing errors
(missing benchmark/esbuild modules and unused locals).

**Ecosystem fact:** `typescript@latest` on npm is now **7.0.2**, the Go native compiler. It has no
`tsserver.js`. "Classic tsserver" means pinning `typescript@6` (installed here as `ts-classic@npm:typescript@6` = 6.0.3).
The `microsoft/typescript-go` repo is archived and work continues in microsoft/TypeScript.
`@typescript/native-preview` (7.0.0-dev.20260707) is a stale nightly. It behaved identically to 7.0.2.

## Files
- `lsp_probe.mjs <tsgo7|preview|tsls6>` is a single LSP harness. It runs the same probes against every server and writes `results/<server>.json` plus the rename WorkspaceEdit.
  Env vars: `WAIT_MS` (sleep before queries), `SYNTAX=never` (tsls `useSyntaxServer`), `OPEN_ALL=1` (didOpen every probed file).
- `apply_edit.mjs results/X.rename.json` applies the WorkspaceEdit, honoring the negotiated encoding. Afterwards, re-run tsc and `git checkout .` in /tmp/zodspike.
- `api_probe.mjs` exercises the TS7 programmatic API (`typescript/unstable/sync`, which spawns `tsc --api`).
- `rust-api-client/` is a dependency-free Rust client (about 80 lines) for the `tsc --api` msgpack protocol. It runs initialize, updateSnapshot, getSymbolAtPosition and typeToString.
- `oxc-probe/` runs oxc 0.154 parser plus semantic over all files.
- `fixtures/alias_probe.ts` (copy it into `/tmp/zodspike/packages/zod/src/`) contains `import { z as zz } from "zod/v4"`, a namespace import, and a line with Cyrillic, CJK and emoji before a `.safeParse` call.

## Results (8-core box, warm page cache unless noted)

| Probe | TS7 LSP (`tsc --lsp --stdio`) | tsserver 6 via typescript-language-server | TS7 `--api` (JS client) | oxc_semantic |
|---|---|---|---|---|
| Setup | 1 static binary, npm platform pkg | node + tsls + pinned TS6; must set `tsserver.path` | same binary; JS client or own msgpack client | cargo crate |
| initialize | 2 ms | 70–210 ms | initialize + project open 120–450 ms (lazy) | n/a |
| First refs (project load + check) | 0.58 s (1.67 s on a cold disk) | 2.7 s | 0.44 s | 0.09–0.17 s for all 333 files |
| Refs `ZodType.safeParse` (iface decl or impl) | 1048 in 79 files, warm 18–40 ms | 1048 in 79 files; 2nd query 39 ms (auto, after load) but about 0.9 s with `never`, twice (it competes with diagnostics) | 1048 in 79 files | not possible (member access is untyped) |
| Refs through `z as zz` and the namespace re-export | yes (both alias_probe sites) | yes | yes | only the local `zz` binding (3 refs) |
| Refs `string()` fn via re-export chains | 2408 / 89 files, 73 ms | 2408 / 89 files, 126 ms | – | – |
| Definition `zz.string` | schemas.ts:617 | same | – | – |
| Hover type | `ZodObject<{a: ZodString; b: ZodNumber}, $strip>` | same | `getTypeAtPosition` + `typeToString` | – |
| Rename `safeParse` | 1048 edits / 79 files, 13–36 ms | identical edit set | no rename API (build it from refs) | – |
| Rename result via tsc | 28 errors become 30 (see gotcha) | same | – | – |
| Incoming calls `clone()` | 14 callers / 26 sites, 23 ms | same, but only if util.ts is didOpen'd | `getSignatureUsage` | – |
| didChange, then refs | +1 ref, 360–740 ms | +1 ref, 1.06 s | `updateSnapshot` + refs: +1, 361 ms | per-file reparse (est. <1 ms, extrapolated) |
| Position encoding | negotiates **utf-8**; verified on a non-ASCII line | utf-16 only | **UTF-16 offsets** (UTF-8 byte offset hit the wrong symbol) | UTF-8 byte spans |
| Peak RSS | 332 MB after refs, 467 MB at end (1 process) | 857 MB, then 1.08 GB (3 procs); 1.28 GB / 4 procs with syntax server | Go child 430–446 MB | 7 MB |

For comparison, `tsc --noEmit` (TS7) takes 10.7 s and 1.55 GB for a full check.

**Timing noise:** a later re-run of `api_probe.mjs` under load average 18 (other jobs on the box) took 0.8 s to load and 2.8–3.9 s for refs. Treat all ms figures as ±3x; the relative ordering held in every run.

**swc (not built):** swc's resolver only applies hygiene `SyntaxContext` marks to identifiers. It has no reference index or module record, so it is strictly less useful than oxc here. This is an expectation and was not measured.

## Gotchas
1. **Silent wrong answers from tsserver during project load.** typescript-language-server defaults to `useSyntaxServer: auto`.
   Early requests are served syntax-only: refs returned 1 hit instead of 1048, hover showed `any`, and definition was empty, all without any error.
   Fix it with `useSyntaxServer: "never"` or by waiting for the load to finish. TS7 blocks until it is correct.
2. tsls rejects requests on files that are not didOpen'd (call hierarchy returned "no item"). TS7 LSP answers for any project file.
3. Both servers' rename misses the string-literal key `vi.spyOn(schema, "safeParse")`. That produced the 2 new type errors.
   semcode should re-type-check after a rename and surface any new errors.
4. Call hierarchy returns nothing for `export const safeParse = _safeParse(...)` (a const whose value is a call). This is the same on both servers.
5. In the TS7 `--api`, reference results are node handles (`index.kind.path`), not positions.
   Mapping them to positions needs the binary AST (`getSourceFile`) decoder, or a JS sidecar. Offsets are UTF-16.
   microsoft/TypeScript#64709 says: the paths drop `/unstable` in 7.1, the JS client API becomes stable through 7.x, but **the wire protocol may change in minor releases**.
   A Rust-native client therefore has to pin the exact TS version.
6. `includeDeclaration: false` differs: TS7 returned 1047, tsserver returned 1048. Normalize this before counting refs.
7. TS7 LSP exposes `custom/initializeAPISession`, which shares the LSP snapshot with an API client. This was not tested.

## Recommendation
Primary backend: **TS7 native LSP** (`tsc --lsp --stdio` from the `typescript` 7.x package), driven from Rust as an LSP client.
It was the fastest of the type-aware options, used the least memory, and had complete refs, rename, call hierarchy and hover parity with tsserver 6.
It negotiates UTF-8 (which matches the Rust/oxc spans), uses a single process, and needs no Node.
Use the `--api` only for checker queries that LSP lacks (assignability, type-graph walks). Reach it via the stable JS client in a Node sidecar, or via the Rust msgpack client pinned to a version.
Use oxc_semantic as a **tier 0** for instant per-file scope/binding refs, the import/export graph, and structural search over local and top-level names.
Never use it for member refs like `.safeParse`, and never for rename.
Keep tsserver 6 only as a fallback for projects that cannot be type-checked by TS7 (e.g. ones relying on TS-API plugins).

**What would reverse it:** a TS7 regression in refs or rename on a corpus with project references or monorepo `paths` (not tested here).
Also a need for tsserver language-service plugins (Vue/Svelte/Angular; TS7 plugin support was not verified here), or LSP-level crashes on large repos.
If the `--api` wire protocol were stabilized, direct Rust-to-API would beat LSP for bulk queries.

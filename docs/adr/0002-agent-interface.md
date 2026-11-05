# ADR-0002: The agent interface

Status: **hypothesis**. Reasoned, not yet measured. To be validated by the interface bake-off (see the eval doc).

## Context

LSP was designed for a human looking at one cursor in one file. Agents work differently:

- They have **no cursor**. They think in names ("the `send` method on `Client`"), not `(line, utf16-col)`.
- They pay for **every token** they read and have a hard context limit. A 600-result reference list is a failure, not a result.
- They **run concurrently** with semcode: the agent may edit a file between asking for a rename preview and applying it.
- They **cannot look at a dialog**. IntelliJ's preview pane has to become data the agent can reason about and then confirm.
- They make **many small calls**, so per-call latency matters more than cold start.

## Decision

### 1. Symbols are addressed by name, positions are secondary
Every symbol gets a **stable, human-readable symbol ID**:

```
rs:grep_searcher::Searcher::search_path
py:httpx._client.Client.send
ts:packages/zod/src/v4/core/schemas.ts#$ZodType
```

Every command that takes a symbol accepts any of: a symbol ID, a fuzzy name (`Client.send`), or a
location (`httpx/_client.py:879:9`). Ambiguous fuzzy names return a short disambiguation list
instead of guessing. Every result includes the symbol ID of its enclosing symbol, so agents can
chain calls without re-searching.

### 2. Output is budgeted, ranked, and paginated
- Every command takes `--budget <tokens>` (default ~2k). The renderer fills the budget in rank
  order, then emits a summary line plus a continuation cursor:
  `… 412 more in 37 files (crates/core: 210, crates/printer: 98, …) — next: semcode more c7f3`.
- Grouping is by file, then by enclosing symbol. Each hit is one line with a minimal snippet.
- The default format is compact text, which is what models read best. `--json` gives the same
  data in structured form, and MCP returns the structured form.

### 3. Edits are two-phase transactions, and stale plans are refused
```
semcode rename py:httpx._client.Client.send send_request
→ plan p_91c2: 14 edits in 5 files (12 certain, 2 uncertain: getattr(...,"send"))
  [unified diff, budgeted]
semcode apply p_91c2 [--only certain]
```
- A plan records a content hash for every file it touches. `apply` **refuses** if any of those
  files changed since the plan was made, and tells the agent to re-plan.
- Every edit is labelled `certain`, `likely`, or `uncertain`, with a reason (dynamic dispatch,
  string reference, macro, `getattr`). Agents can apply a subset.
- After `apply`, semcode reports **new diagnostics** in the touched files (the delta, not the
  full list). That gives the agent the "red squiggle" feedback a human would see.
- Applied changes are recorded in a journal, so `semcode undo <plan>` works.

### 4. IntelliJ capabilities become agent verbs
| IntelliJ | semcode verb |
|---|---|
| Structural Search / Replace | `find` / `rewrite` (patterns with typed metavariables, preview by default) |
| Find Usages | `refs` (alias-proof, with read/write/call/import classification) |
| Call Hierarchy | `callers` / `callees` (depth, budget) |
| Data flow to/from here | `flow --to` / `flow --from` |
| Rename / Change Signature / Move | `rename` / `change-sig` / `move` (plans) |
| Alt+Enter intentions & quick fixes | `fixes <loc>` lists actions, `fix <action-id>` produces a plan |
| File structure / Navigate symbol | `outline <file|dir>` / `symbols <query>` |
| Quick documentation / type info | `info <symbol|loc>`: signature, type, docs, defining file |
| Search Everywhere (meaning) | `ask "<natural language>"` |

Agents also get verbs that humans don't need:
- `read <symbol>` returns exactly the source of a symbol (and optionally its dependencies'
  signatures), so the agent doesn't have to read the whole file.
- `edit <symbol> --body <file|stdin>` replaces a symbol's body or definition by ID, with no
  line-number arithmetic. It produces a plan like every other edit.

### 5. Process model
A per-workspace daemon (`semcoded`) holds the indexes and the language backends warm. `semcode`
(CLI) and `semcode mcp` (MCP server over stdio) are thin clients. The daemon starts on first use
and stops after an idle timeout. If the daemon can't start, the CLI falls back to in-process
execution for L1 commands.

### 6. Positions
Internally everything is **UTF-8 byte offsets** (what tree-sitter and Rust strings use). We convert
to `line:col` at the edges. Columns shown to agents are 1-based **character** columns. LSP
backends negotiate `positionEncoding: utf-8` when the server supports it, and otherwise we convert
through a per-file line index.

## Consequences
- We need a symbol-ID scheme that works the same way across three languages (ADR to follow, after
  the backend spikes).
- Budgeted rendering is a core module, not an afterthought, and every command's output goes
  through it.
- The plan/apply/journal machinery is shared by every refactoring verb, and gets built before
  any individual refactoring.

## What would change this decision
- If agents in practice ignore cursors and re-search instead of paginating, we drop pagination
  in favour of better ranking and summaries.
- If MCP gains native streaming/pagination primitives, we use them instead of cursors.

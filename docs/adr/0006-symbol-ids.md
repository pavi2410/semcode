# ADR-0006: Symbol IDs

Status: accepted (2026-10-14)

## Context
Agents hold on to symbol IDs across many calls and across edits (ADR-0002). M1 (syntax only)
already hands IDs out through `outline`, `read`, `edit` and `find`. M2 (oracles) must keep exactly
those IDs working, or every ID an agent holds breaks when we upgrade.

## Decision

### Rule 1: IDs are computed from the syntax tree, never from oracle output
L1 computes the canonical ID of every **definition** from its file path and its syntactic nesting.
When an oracle returns a location, L2 maps it back to the innermost L1 definition whose name range
contains it. We never parse an oracle's display name into an ID. That way an ID exists without any
backend running, and it doesn't change when an oracle is upgraded.

### Rule 2: grammar
```
symbol-id := lang ":" module [ "#" path ]
lang      := "rs" | "py" | "ts"
module    := language-native module path        (see below)
path      := segment { "." segment }
segment   := name [ "@" disambiguator ]
name      := identifier | quoted-string | "<" type " as " trait ">" | "{" anon-kind "#" n "}"
```
- `#` separates "where the file is" from "what's inside it". `#` is not valid in identifiers of
  any of the three languages. (`$` is valid in TS, which is why we don't use `$`.)
- No line numbers, no hashes. An ID survives every edit that doesn't rename, move or re-nest the symbol.
- Every ID is a valid input to every command, and fuzzy forms (`Client.send`, `find_at`) resolve
  to a list of IDs.

### Rust
- **module** = `<crate>::<mod>::<mod>`. `<crate>` is the Cargo target name (`[lib] name` or package
  name with `-` → `_`; binaries use the `[[bin]] name`). Modules come from the file layout
  (`src/foo/mod.rs` and `src/foo.rs` → `foo`) plus inline `mod x { }` blocks. Files outside
  `src/` (`tests/`, `benches/`, `examples/`) are their own targets: `grep_matcher[test=util]`.
- **Inherent methods:** `rs:grep_searcher::searcher#Searcher.search_path`
- **Trait items:** `rs:grep_matcher#Matcher.find_at`
- **Trait-impl members** use Rust's qualified-path syntax with generics erased:
  `rs:grep_regex::matcher#<RegexMatcher as Matcher>.find_at`.
  `impl<M: Matcher, W> Sink for StandardSink<'_, M, W>` → `<StandardSink as Sink>`.
- **Free functions:** `rs:rg::search#search_path` (ripgrep's `crates/core/main.rs` is the `[[bin]] rg` target).
- **Nested items** inside fn bodies: `outer.Inner`. **Closures:** `outer.{closure#1}`.
- **Macro-generated items** have no L1 ID. Oracle locations inside a macro call map to the
  invoking item, flagged `via-macro`.

### Python
- **module** = the dotted import path, found by walking up from the file while `__init__.py`
  exists. Source roots are detected from `pyproject.toml` (`[tool.*] packages`/`src` layouts),
  with `src/` as a root if present. Files outside any package (e.g. `tests/client/test_auth.py`
  with no `__init__.py`) use their path relative to the project root, dotted:
  `tests.client.test_auth`.
- `py:httpx._client#Client.send`, `py:httpx._client#Client._send_handling_auth`,
  nested `outer.inner`, `{lambda#1}`, and module variables `py:httpx._config#DEFAULT_TIMEOUT_CONFIG`.
- **Overloads:** all `@overload` stubs plus the implementation make up **one** symbol, and the
  ID refers to the group. A property's getter, setter and deleter likewise make up one symbol.
- **Conditional redefinitions** (`try:/except ImportError:` and `if TYPE_CHECKING:`) also make up
  one symbol with several definition sites.

### TypeScript
- **module** = the file path relative to the workspace root, without the extension:
  `ts:packages/zod/src/v4/core/schemas`. We use the path rather than the package name because
  monorepos, `paths` and conditional exports make package names ambiguous. The package
  name is shown in the output for context.
- `ts:packages/zod/src/v4/classic/schemas#ZodType.safeParse`
- **Statics vs instance members** with the same name: the static gets `@static`.
- **Overloads** and **declaration merging** (`interface X` + `class X`, namespace merging) make
  up one symbol with several definition sites.
- **Default exports:** the local name if there is one, otherwise `default`.
- **Object-literal members:** `const api = { parse() {} }` → `api.parse`.
- **Anonymous functions:** `{arrow#1}` and `{fn#1}`, numbered in source order within the parent.
- **Computed or quoted members:** `Foo."content-type"`.
- **Re-exports are references, not symbols.** `export { a as b }` never gets its own ID; it shows
  up in `refs` with the role `re-export as b`.

### Collisions
If two definitions would get the same ID, the second and later ones get `@2`, `@3`… in source
order. Cases: `cfg`-gated duplicate fns, two `impl Foo` blocks with the same method, Python
redefinitions outside the grouped cases above. Ordinals are the only fragile part. Results
mark them so agents know to re-resolve after edits.

## Consequences
- `semcode-lang` needs a per-language "definition walker" that emits `(id, kind, name_range,
  full_range, parent_id)`. `outline`, `read` and `edit` are thin layers over it.
- Mapping oracle locations to IDs is a lookup in an interval tree per file.
- Renames and moves change IDs on purpose. A successful `rename`/`move` plan reports
  `old-id → new-id` so agents can update the IDs they're holding.

## Open questions
- **Workspace-level crate naming:** if a workspace has two targets with the same name, add the
  package (`ripgrep/rg::search`)? Decide once we see a real conflict.
- **Python namespace packages without `__init__.py`:** we follow the PEP 420 heuristic of using
  the configured source root. To be validated on a real project.

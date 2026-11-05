# Spike: Structural Search & Replace (SSR) for semcode

Question: should IntelliJ-style SSR (code-shaped patterns, `$X` / `$$$XS` metavariables,
syntactic and **semantic** constraints, and formatting-preserving idempotent replacement) be
built on `ast-grep-core`, on a fork of it, or on our own engine over tree-sitter?

## Layout / how to run

```
cargo build --release
./target/release/ssr_spike demo      # fixtures/*: all test patterns through A and B, backtracking probes, ambiguity probes
./target/release/ssr_spike quality   # rewrites over corpus/{ripgrep,httpx,zod}: parse errors, idempotence, lost comments
./target/release/ssr_spike bench     # throughput, parse and match timed separately, 5 iterations
./target/release/ssr_spike query     # option C: tree-sitter query with a custom #sem-type? predicate
./target/release/ssr_spike extra     # TSX, rewrite-path throughput, adversarial $$$ backtracking
./target/release/ssr_spike rewrite-file <a|b> <file> '<pattern>' '<template>'
```

- `src/agrep.rs` (110 LOC): option A, ast-grep-core used as a library, plus a semantic-predicate bridge.
- `src/engine.rs` (~690 LOC): option B, our own engine.
  - matcher: about 300 LOC
  - template renderer: about 200 LOC
  - token-LCS trivia pass: about 170 LOC
- `src/query.rs`: option C.
- `src/view.rs`: the engine-neutral predicate hook `(Ctx{path, semantic db}, NodeView{kind, text, byte range}) -> bool`, plus a fake type oracle.
- `results/*.txt`: captured output of the runs above.

Versions:

| Crate | Version |
|---|---|
| ast-grep-core | 0.50.0 |
| ast-grep-language | 0.50.0 (features: rust, python, typescript) |
| tree-sitter | 0.27.1 |
| tree-sitter-rust | 0.24.2 |
| tree-sitter-python | 0.25.0 |
| tree-sitter-typescript | 0.23.2 (ts + tsx) |

Toolchain: rustc 1.98.

## Findings

### A. ast-grep-core as a library

Good:
- It supports Rust, Python, TS and TSX out of the box, using an expando char `µ` for the `$` in Rust and Python.
- Pattern parsing and root unwrapping are mature. `Pattern::contextual(ctx, selector)` handles ambiguous fragments: `$K: $V` matches nothing bare, but matches with context `const o = { $K: $V }` and selector `pair`.
- Smart strictness distinguishes `==` from `===`. `Signature` strictness matches `===` and `!=` as well.
- Indentation-aware templates work for Python (`if`-None to a conditional expression and back, inside a class method).
- Comments inside `$$$ARMS` are kept.
- Speed is about the same as B.

Bad:
- **`$$$` does not backtrack.** The ellipsis stops at the first candidate the next pattern node *probes* as matching, and the choice is never revisited (`match_tree/match_node.rs::may_match_ellipsis_impl`). So `f($$$A, $B)` does not match `f(1, 2)`, nor any call with two or more arguments. The same happens for `f($$$A, 3)` on `f(3,1,3)` and `f($$$A,$B,$B)` on `f(1,2,2)`.
- **Constraints are post-hoc.** `Matcher::match_node_with_env` is generic over `D: Doc`, and `Op::func`/`Predicate` is commented out in 0.50. A closure can only be attached through a wrapper `Matcher` that type-erases to `NodeView` and runs *after* the structural match (this works; see `agrep::Bound`). The consequences:
  - A failing semantic predicate cannot steer the `$$$` split: `[$$$X, $Y, $$$Z]` with `$Y==2` on `[1,2,3]` gives 0 matches.
  - Multi-captures are invisible to `MetaVarEnv::match_constraints`.
  - ast-grep-config 0.50 does the same: `rule_core.rs::do_match` calls `match_constraints` after `rule.match_node_with_env`. I checked this in the source.
- **`replace_all` is non-reentrant.** The rewrite has to be iterated to a fixpoint. On the corpus, 27 nested `unwrap` matches and 10 nested `match` matches remain after one pass.
- **Template list handling is naive:**
  - An empty `$$$ARGS` gives `requests.get(, timeout=10)`, which is a parse error.
  - A trailing comma gives `...,, timeout=10)`, a parse error. Seen on `httpx/tests/test_exceptions.py` in a run *without* the no-timeout guard, so it also duplicated `timeout=`; the `,,` is the finding.
  - A trailing `# comment` in the capture swallows the inserted `, timeout=10)`.
- Indentation is re-based, but layout is not preserved. Multi-line calls collapse (`get(\n url` becomes `get(url`), and blank lines inside `$$$` get trailing whitespace.
- An `if` block that contains a comment does not match under Smart strictness (1 of 2 matches in the fixture).
- `'$MOD'` does not match `"zod"`: the quote style is significant.
- The parts you would need to change (`match_tree`, `MetaVarEnv` internals) are private, so any fix means a fork.

### B. Own engine (tree-sitter, same grammars)

How it works:
- **Patterns.** `$X` becomes `__ssr_X` and `$$$XS` becomes `__ssrm_XS`. The pattern is parsed by the same grammar, wrapped in a context when needed (Rust tries `fn _(){ … }`). An ERROR node consisting only of a placeholder is tolerated, so `match $E { $$$ARMS }` parses.
- **Matching.** Unification runs over non-extra children with full backtracking over `$$$`. **Predicates run at bind time inside the backtracking.** Repeated metavariables use structural equality. Separators next to an empty `$$$` are optional. Quote style is ignored.
- **Replacement.** The template renderer re-indents captured text and elides the comma next to an empty `$$$`. Nested matches are rewritten in a single pass.
- **Trivia pass.** After rendering, a token-level LCS against the original keeps the *original* whitespace and comments wherever the tokens around them survived, re-indented relative to the anchor line. It reports any comment it has to drop.

Results:
- TSX works in both engines (`fixtures/sample.tsx`: 2 of 2 `==` inside JSX were rewritten).
- **A pattern must be compiled per grammar.** The TS-compiled `$A == $B` matches 0 on a TSX tree in *both* engines because kind ids differ, so dispatch by extension: `.ts` to TypeScript, `.tsx` to Tsx.
- All backtracking probes match.
- Kwarg insertion gives `headers={...}, timeout=10  # keep me` with the original layout kept.
- Multi-line method chains keep `.expect(..)` on their own line.
- On the corpus, every rewrite reaches a fixpoint in one pass, with no new parse errors.
- Idempotence is a property of rule plus guard. For example, `match &$E` is idempotent only because of the `$E` kind is not `reference_expression` predicate. What the engine contributes is reentrant single-pass rewriting, so nested matches are not left behind.
- Remaining gap: a comment inside a rewritten region is dropped. The engine reports it (1 in the fixture), which is a natural place for a "needs review" flag.

Known risk: unmemoized backtracking is polynomial in the number of `$$$`. The pattern `f($$$A, 1, $$$B, 2, $$$C, 99)` on a 400-argument call takes 354–729 ms to fail in B across two loaded runs, against 0.6–1.4 ms in A (A is fast only because it doesn't backtrack). Mitigations:
- memoize failures on (pattern index, candidate index) when no new bindings were made
- a per-match step budget
- an anchor prefilter that requires literal tokens such as `99` to be present

The trivia pass reparses the output and runs a dense LCS per top-level match. On ripgrep it costs about 2x on the rewrite path (`results/extra.txt`; two runs on the loaded machine):

| Rule | A | B, no trivia pass | B, with trivia pass |
|---|---|---|---|
| `match` | 242–257 files/s | 261–397 files/s | 123–217 files/s |
| `unwrap` | 246–437 files/s | 251–484 files/s | 107–218 files/s |

Fine for interactive use. Reparsing incrementally and limiting LCS to the changed window would remove most of the cost.

Hard parts actually hit:
- error-tolerant fragment parsing
- trailing `;` and the replace range
- separators next to an empty `$$$`
- `'` and `"` being different anonymous kinds
- the gap re-indent rules

Each was small, and all of them are well inside the ~600 LOC budget.

### C. tree-sitter queries

Unknown predicates (`#sem-type? @x "io::Result"`) come back through `Query::general_predicates()`, so semantic hooks are easy. The demo finds 5 syntactic `unwrap()` matches and keeps 1 after the semantic filter. Queries are not code-shaped and have no `$$$`-with-unification, so they are only useful as an escape hatch and as the compile target for `kind`/`has`/`inside`-style rules.

## Measurements

`results/bench.txt`, release build, 5 iterations, 2 patterns per language. The machine was at **load average about 17 on 8 cores** during the runs, so absolute numbers are pessimistic and runs vary by about ±30%.

| Corpus | A total files/s | B total files/s | A match-only | B match-only |
|---|---|---|---|---|
| ripgrep (110 rs, 1.9 MB) | 202 | 195 | 666 | 753 |
| httpx (60 py, 0.6 MB) | 232 | 170 | 982 | 738 |
| zod (515 ts, 3.6 MB) | 257 | 261 | 982 | 1145 |

- Parsing takes about 70–75% of the time, and both engines use the same parser. Matching speed is a wash, so with cached trees from the index SSR is never the bottleneck.
- Both engines return identical match counts on the shared patterns: unwrap 1047, match 356, httpx.get 6, if-None 4, == 3.
- Only B matched zod's double-quoted imports (588 vs 0).
- The corpus `import +.js` rewrite is vacuous: zod already uses `.js` extensions, so both engines report 0. The fixture is the evidence for that case.

## Recommendation

**Build our own engine (B).** Reuse ast-grep only as a reference, and optionally `ast-grep-language`'s expando and pre-process tricks. The decisive points:
- Semantic predicates must take part in unification and backtracking.
- Our replacements need a minimal-diff trivia pass.

Neither point fits ast-grep's public API. A fork would mean rewriting `may_match_ellipsis_impl` as a backtracking matcher and threading a type-erased predicate through `MetaVarEnv`/`Aggregator`. By my *estimate* from reading `match_node.rs` (I did not attempt the patch), that is roughly 200–400 LOC of changes in private hot code, rebased against a fast-moving 0.x crate. That is about the size of our whole matcher.

What would reverse this:
- ast-grep upstream accepts or ships a backtracking ellipsis plus an in-match predicate hook. Then use it as a library and keep only our replacement and trivia layer.
- We also need YAML rule composition (`inside`/`has`/`follows`, transforms, rewriters) from `ast-grep-config` quickly and cannot rebuild it.
- Real workloads only ever use single-node constraints, where post-hoc filtering is fine.

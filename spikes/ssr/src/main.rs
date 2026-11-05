mod agrep;
mod engine;
mod query;
mod view;

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::{Python, Rust, Tsx, TypeScript};
use std::time::{Duration, Instant};
use view::{Ctx, NodeView};

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");
const CORPUS: &str = "/home/pavitra/Playground/semcode/corpus";

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or("demo".into());
    match cmd.as_str() {
        "demo" => demo(),
        "bench" => bench(),
        "query" => query::demo(),
        "quality" => quality(),
        "extra" => extra(),
        "rewrite-file" => {
            // ssr_spike rewrite-file <a|b> <file> <pattern> <template>
            let a: Vec<String> = std::env::args().collect();
            let src = std::fs::read_to_string(&a[3]).unwrap();
            let ctx = Ctx::new(&a[3]);
            let ext = a[3].rsplit('.').next().unwrap();
            if a[2] == "b" {
                let spec = match ext { "rs" => engine::rust(), "py" => engine::python(), "tsx" => engine::tsx(), _ => engine::ts() };
                let p = engine::Pat::compile(&spec, &a[4]).unwrap();
                print!("{}", engine::rewrite(&spec, &p, &ctx, &src, &a[5]).0);
            } else {
                let out = match ext {
                    "rs" => agrep::rewrite(&agrep::Rule::new(&a[4], Rust).unwrap(), &ctx, &src, Rust, &a[5]).0,
                    "py" => agrep::rewrite(&agrep::Rule::new(&a[4], Python).unwrap(), &ctx, &src, Python, &a[5]).0,
                    _ => agrep::rewrite(&agrep::Rule::new(&a[4], TypeScript).unwrap(), &ctx, &src, TypeScript, &a[5]).0,
                };
                print!("{out}");
            }
        }
        "sexp" => { let a: Vec<String> = std::env::args().collect(); let l = match a[2].as_str() { "rs" => engine::rust(), "py" => engine::python(), _ => engine::ts() }; print!("{}", engine::debug_sexp(&l, &a[3])); for s in [&a[4]] { let t = engine::parse(&l.ts, s); println!("target: {}", t.root_node().to_sexp()); let p = engine::Pat::compile(&l, &a[3]).unwrap(); println!("{:?}\ncount={}", p.root, engine::count(&l, &p, &view::Ctx::new("x"), s)); } }
        _ => eprintln!("usage: ssr_spike [demo|bench|query]"),
    }
}

fn read(f: &str) -> String {
    std::fs::read_to_string(format!("{FIX}/{f}")).unwrap()
}

fn show(title: &str, before: &str, after: &str, n: usize, err: bool, second: usize) {
    println!("--- {title}: {n} rewrites, parse-error-after={err}, matches-on-second-pass={second}");
    if before != after {
        for d in diff_lines(before, after) {
            println!("{d}");
        }
    }
}

fn diff_lines(a: &str, b: &str) -> Vec<String> {
    // tiny line diff: print changed region
    let (al, bl): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    let mut s = 0;
    while s < al.len() && s < bl.len() && al[s] == bl[s] {
        s += 1;
    }
    let (mut ea, mut eb) = (al.len(), bl.len());
    while ea > s && eb > s && al[ea - 1] == bl[eb - 1] {
        ea -= 1;
        eb -= 1;
    }
    let mut out = vec![];
    for l in &al[s..ea] {
        out.push(format!("  - {l}"));
    }
    for l in &bl[s..eb] {
        out.push(format!("  + {l}"));
    }
    out
}

// ---------------- predicates shared by both engines ----------------
fn no_timeout_kw(_: &Ctx, args: &[NodeView]) -> bool {
    !args.iter().any(|a| a.kind == "keyword_argument" && a.text.starts_with("timeout"))
}
fn not_ref(_: &Ctx, e: &NodeView) -> bool {
    e.kind != "reference_expression"
}
fn relative_no_ext(_: &Ctx, m: &NodeView) -> bool {
    m.text.starts_with('.') && !m.text.ends_with(".js")
}
fn is_result(ctx: &Ctx, x: &NodeView) -> bool {
    ctx.type_of(x).is_some_and(|t| t.starts_with("io::Result"))
}

/// Fake semantic oracle: pretend a type checker told us `File::open(..)` is io::Result<File>.
fn rust_ctx(src: &str) -> Ctx {
    let mut c = Ctx::new("sample.rs");
    for (i, _) in src.match_indices("File::open") {
        c.types.insert(("sample.rs".into(), i), "io::Result<File>".into());
    }
    c
}

struct Case {
    name: &'static str,
    file: &'static str,
    pat: &'static str,
    tpl: &'static str,
}

fn demo() {
    let rs = read("sample.rs");
    let py = read("sample.py");
    let tss = read("sample.ts");
    let rctx = rust_ctx(&rs);
    let pctx = Ctx::new("sample.py");
    let tctx = Ctx::new("sample.ts");

    let cases = [
        Case { name: "rust unwrap->expect (all)", file: "rs", pat: "$X.unwrap()", tpl: "$X.expect(\"TODO\")" },
        Case { name: "rust unwrap->? when type_of($X)=io::Result (semantic)", file: "rs", pat: "$X.unwrap()", tpl: "$X?" },
        Case { name: "rust match &$E (multi-line $$$ARMS, comments)", file: "rs", pat: "match $E { $$$ARMS }", tpl: "match &$E {\n    $$$ARMS\n}" },
        Case { name: "py add timeout kwarg", file: "py", pat: "requests.get($$$ARGS)", tpl: "requests.get($$$ARGS, timeout=10)" },
        Case { name: "py if-None -> conditional expr", file: "py", pat: "if $V is None:\n    $V = $D", tpl: "$V = $D if $V is None else $V" },
        Case { name: "py conditional expr -> if-None (multi-line out)", file: "py", pat: "$V = $D if $V is None else $V", tpl: "if $V is None:\n    $V = $D" },
        Case { name: "ts == -> ===", file: "ts", pat: "$A == $B", tpl: "$A === $B" },
        Case { name: "ts import add .js to relative", file: "ts", pat: "import { $$$NAMES } from '$MOD'", tpl: "import { $$$NAMES } from '$MOD.js'" },
    ];

    println!("================ OPTION A: ast-grep-core 0.50 ================");
    for (i, c) in cases.iter().enumerate() {
        let (src, ctx) = match c.file { "rs" => (&rs, &rctx), "py" => (&py, &pctx), _ => (&tss, &tctx) };
        macro_rules! run {
            ($lang:expr) => {{
                let rule = match agrep::Rule::new(c.pat, $lang) {
                    Ok(r) => r,
                    Err(e) => { println!("--- {}: PATTERN ERROR {e}", c.name); continue; }
                };
                let rule = match i {
                    1 => rule.with("X", is_result),
                    2 => rule.with("E", not_ref),
                    3 => rule.with_multi("ARGS", no_timeout_kw),
                    7 => rule.with("MOD", relative_no_ext),
                    _ => rule,
                };
                let found = agrep::count(&rule, ctx, src, $lang);
                let (out, n) = agrep::rewrite(&rule, ctx, src, $lang, c.tpl);
                let err = agrep::root_has_error(&out, $lang);
                let second = agrep::count(&rule, ctx, &out, $lang);
                println!("(found {found})");
                show(c.name, src, &out, n, err, second);
            }};
        }
        match c.file { "rs" => run!(Rust), "py" => run!(Python), _ => run!(TypeScript) }
    }

    println!("\n================ OPTION B: own engine ================");
    let (lr, lp, lt) = (engine::rust(), engine::python(), engine::ts());
    for (i, c) in cases.iter().enumerate() {
        let (src, ctx, lang) = match c.file { "rs" => (&rs, &rctx, &lr), "py" => (&py, &pctx, &lp), _ => (&tss, &tctx, &lt) };
        let pat = match engine::Pat::compile(lang, c.pat) {
            Ok(p) => p,
            Err(e) => { println!("--- {}: PATTERN ERROR {e}", c.name); continue; }
        };
        let pat = match i {
            1 => pat.with("X", is_result),
            2 => pat.with("E", not_ref),
            3 => pat.with_multi("ARGS", no_timeout_kw),
            7 => pat.with("MOD", relative_no_ext),
            _ => pat,
        };
        let found = engine::count(lang, &pat, ctx, src);
        let (plain, _) = engine::rewrite_plain(lang, &pat, ctx, src, c.tpl);
        let (out, n, dropped) = engine::rewrite(lang, &pat, ctx, src, c.tpl);
        let err = engine::has_error(&lang.ts, &out);
        let second = engine::count(lang, &pat, ctx, &out);
        println!("(found {found}, ctx#{}, comments dropped={dropped}, trivia-pass changed output={})", pat.context_used, plain != out);
        show(c.name, src, &out, n, err, second);
    }

    println!("\n================ BACKTRACKING / CONSTRAINT-STEERING ================");
    let probes: [(&str, &str, Option<(&str, &str)>); 8] = [
        ("f($$$A, $B)", "f(1)", None),
        ("[$$$X, $Y, $$$Z]", "[1, 2, 3]", None),
        ("f($$$A, $B)", "f(1, \"x\", 2)", None),
        ("[$$$X, $Y, $$$Z]", "[1, 2, 3]", Some(("Y", "2"))),
        ("f($$$A, 3)", "f(3, 1, 3)", None),
        ("f($$$A, $B, $B)", "f(1, 2, 2)", None),
        ("f($$$A, $B, $$$C)", "f(a, b, c)", Some(("B", "b"))),
        ("f($$$A, $B, $$$C)", "f(a, b, c)", None),
    ];
    for (p, s, pred) in probes {
        let mk_a = || {
            let r = agrep::Rule::new(p, TypeScript).unwrap();
            match pred { Some((v, t)) => r.with(v, move |_, n| n.text == t), None => r }
        };
        let a = agrep::count(&mk_a(), &tctx, s, TypeScript);
        let bp = engine::Pat::compile(&lt, p).unwrap();
        let bp = match pred { Some((v, t)) => bp.with(v, move |_, n| n.text == t), None => bp };
        let b = engine::count(&lt, &bp, &tctx, s);
        println!("pattern {p:<22} on {s:<16} pred={pred:?}: ast-grep={a} own={b}");
    }

    println!("\n================ PATTERN PARSE AMBIGUITY ================");
    for (p, s, tag) in [
        ("foo($A)", "foo(1); let x = foo(2);", "rs"),
        ("$A.unwrap()", "x.unwrap();", "rs"),
        ("{ $$$B }", "let o = { a: 1 }; if (x) { y(); }", "ts"),
        ("$K: $V", "const o = { a: 1 }", "ts"),
        ("$X = $Y", "a = 1\nb: int = 2", "py"),
        ("print($A)", "print(1)", "py"),
    ] {
        let a = match tag {
            "rs" => agrep::Rule::new(p, Rust).map(|r| agrep::count(&r, &tctx, s, Rust)),
            "py" => agrep::Rule::new(p, Python).map(|r| agrep::count(&r, &tctx, s, Python)),
            _ => agrep::Rule::new(p, TypeScript).map(|r| agrep::count(&r, &tctx, s, TypeScript)),
        };
        let lang = match tag { "rs" => &lr, "py" => &lp, _ => &lt };
        let b = engine::Pat::compile(lang, p).map(|bp| engine::count(lang, &bp, &tctx, s));
        println!("[{tag}] {p:<12} on {s:?}: ast-grep={a:?} own={b:?}");
    }
    // ast-grep's answer to fragment ambiguity: contextual pattern + kind selector
    let cp = ast_grep_core::Pattern::contextual("const o = { $K: $V }", "pair", TypeScript).unwrap();
    let r = agrep::Rule { pattern: cp, single: vec![], multi: vec![] };
    println!("ast-grep contextual('const o = {{ $K: $V }}', selector=pair) on 'const o = {{ a: 1 }}': {}", agrep::count(&r, &tctx, "const o = { a: 1 }", TypeScript));
    // strictness note: ast-grep Relaxed / Signature
    for st in [ast_grep_core::MatchStrictness::Smart, ast_grep_core::MatchStrictness::Signature, ast_grep_core::MatchStrictness::Template] {
        let mut r = agrep::Rule::new("$A == $B", TypeScript).unwrap();
        r.pattern = r.pattern.with_strictness(st.clone());
        let n = agrep::count(&r, &tctx, "a == b; c === d; e != f;", TypeScript);
        println!("ast-grep strictness {:?}: `$A == $B` matches {n} of [a == b; c === d; e != f]", strictness_name(&st));
    }
}

fn strictness_name(s: &ast_grep_core::MatchStrictness) -> &'static str {
    use ast_grep_core::MatchStrictness::*;
    match s { Cst => "Cst", Smart => "Smart", Ast => "Ast", Relaxed => "Relaxed", Signature => "Signature", Template => "Template" }
}

// ---------------- TSX, rewrite-path timing, adversarial backtracking ----------------

fn extra() {
    let ctx = Ctx::new("x");
    let src = read("sample.tsx");
    // TSX through both engines
    let r = agrep::Rule::new("$A == $B", Tsx).unwrap();
    let (out, n) = agrep::rewrite(&r, &ctx, &src, Tsx, "$A === $B");
    println!("TSX A: {n} rewrites, err={} -> {}", agrep::root_has_error(&out, Tsx), out.trim());
    let r_ts = agrep::Rule::new("$A == $B", TypeScript).unwrap();
    // pattern compiled with TS grammar applied to a TSX tree (kind ids differ?)
    use ast_grep_core::matcher::MatcherExt;
    let root = Tsx.ast_grep(src.as_str());
    let cross = root.root().dfs().filter(|n| r_ts.pattern.match_node(n.clone()).is_some()).count();
    println!("TSX A: TS-compiled pattern on TSX tree matches {cross}");
    let tsx = engine::tsx();
    let p = engine::Pat::compile(&tsx, "$A == $B").unwrap();
    let (out, n, d) = engine::rewrite(&tsx, &p, &ctx, &src, "$A === $B");
    println!("TSX B: {n} rewrites, dropped={d}, err={} -> {}", engine::has_error(&tsx.ts, &out), out.trim());
    let p_ts = engine::Pat::compile(&engine::ts(), "$A == $B").unwrap();
    println!("TSX B: TS-compiled pattern on TSX tree matches {}", engine::count(&tsx, &p_ts, &ctx, &src));
    let p_ts = engine::Pat::compile(&engine::ts(), "import { $$$N } from '$M'").unwrap();
    let p_tsx = engine::Pat::compile(&tsx, "import { $$$N } from '$M'").unwrap();
    let isrc = "import { a } from './m';";
    println!("TSX B: TS-compiled import pattern on TSX tree: {}, TSX-compiled: {}", engine::count(&tsx, &p_ts, &ctx, isrc), engine::count(&tsx, &p_tsx, &ctx, isrc));

    // rewrite-path throughput: ripgrep `match` rule and unwrap rule, full rewrite incl. B's trivia pass
    let fs = files();
    let rs: Vec<&String> = fs.iter().filter(|(l, _)| *l == L::Rs).map(|(_, s)| s).collect();
    for (pat, tpl) in [("match $E { $$$ARMS }", "match &$E {\n    $$$ARMS\n}"), ("$X.unwrap()", "$X.expect(\"TODO\")")] {
        let t = Instant::now();
        let ra = agrep::Rule::new(pat, Rust).unwrap();
        for s in &rs { let _ = agrep::rewrite(&ra, &ctx, s, Rust, tpl); }
        let ta = t.elapsed();
        let spec = engine::rust();
        let pb = engine::Pat::compile(&spec, pat).unwrap();
        let t = Instant::now();
        for s in &rs { let _ = engine::rewrite_plain(&spec, &pb, &ctx, s, tpl); }
        let tbp = t.elapsed();
        let t = Instant::now();
        for s in &rs { let _ = engine::rewrite(&spec, &pb, &ctx, s, tpl); }
        let tb = t.elapsed();
        let fps = |d: Duration| rs.len() as f64 / d.as_secs_f64();
        println!("rewrite ripgrep `{pat}`: A {:.0} files/s, B(no trivia) {:.0} files/s, B(+trivia pass) {:.0} files/s  (incl. parse)", fps(ta), fps(tbp), fps(tb));
    }

    // adversarial backtracking: k `$$$` over an n-arg call that cannot match
    let ts = engine::ts();
    for (pat, n) in [("f($$$A, $B, $$$C, 99)", 200), ("f($$$A, 1, $$$B, 2, $$$C, 99)", 200), ("f($$$A, 1, $$$B, 2, $$$C, 99)", 400)] {
        let args: Vec<String> = (0..n).map(|i| ((i % 3) as u32).to_string()).collect();
        let src = format!("f({});", args.join(", "));
        let p = engine::Pat::compile(&ts, pat).unwrap();
        let t = Instant::now();
        let c = engine::count(&ts, &p, &ctx, &src);
        let tb = t.elapsed();
        let ra = agrep::Rule::new(pat, TypeScript).unwrap();
        let t = Instant::now();
        let ca = agrep::count(&ra, &ctx, &src, TypeScript);
        println!("adversarial {pat} on {n} args: B {c} matches in {:?}; A {ca} in {:?}", tb, t.elapsed());
    }
}

// ---------------- rewrite quality on the corpora ----------------

fn quality() {
    let fs = files();
    type Pred = Option<fn(&mut agrep::Rule) -> ()>;
    let _ : Pred = None;
    let cases: [(L, &str, &str, &str, u8); 6] = [
        (L::Rs, "unwrap->expect", "$X.unwrap()", "$X.expect(\"TODO\")", 0),
        (L::Rs, "match &$E", "match $E { $$$ARMS }", "match &$E {\n    $$$ARMS\n}", 2),
        (L::Py, "httpx.get +timeout", "httpx.get($$$ARGS)", "httpx.get($$$ARGS, timeout=10)", 3),
        (L::Py, "if-None->cond", "if $V is None:\n    $V = $D", "$V = $D if $V is None else $V", 0),
        (L::Ts, "== -> ===", "$A == $B", "$A === $B", 0),
        (L::Ts, "import +.js", "import { $$$NAMES } from '$MOD'", "import { $$$NAMES } from '$MOD.js'", 7),
    ];
    let ctx = Ctx::new("corpus");
    for (lang, name, pat, tpl, pred) in cases {
        let set: Vec<(L, &String)> = fs.iter().filter(|(l, _)| *l == lang || (lang == L::Ts && *l == L::Tsx)).map(|(l, s)| (*l, s)).collect();
        // A
        let (mut rw, mut files_changed, mut new_err, mut second, mut comments_lost) = (0, 0, 0, 0, 0);
        for (l, src) in &set {
            macro_rules! go {
                ($lg:expr) => {{
                    let mut r = agrep::Rule::new(pat, $lg).unwrap();
                    r = match pred { 2 => r.with("E", not_ref), 3 => r.with_multi("ARGS", no_timeout_kw), 7 => r.with("MOD", relative_no_ext), _ => r };
                    let (out, n) = agrep::rewrite(&r, &ctx, src, $lg, tpl);
                    if n > 0 {
                        rw += n; files_changed += 1;
                        if agrep::root_has_error(&out, $lg) && !agrep::root_has_error(src, $lg) { new_err += 1; }
                        second += agrep::count(&r, &ctx, &out, $lg);
                        comments_lost += count_comments(src).saturating_sub(count_comments(&out));
                    }
                }};
            }
            match (lang, *l) { (L::Rs, _) => go!(Rust), (L::Py, _) => go!(Python), (_, L::Tsx) => go!(Tsx), _ => go!(TypeScript) }
        }
        println!("A {name:<20} rewrites={rw:<5} files={files_changed:<4} files-with-new-parse-errors={new_err:<3} matches-left-after-1-pass={second:<4} comments-lost={comments_lost}");
        // B
        let (mut rw, mut files_changed, mut new_err, mut second, mut comments_lost, mut reported) = (0, 0, 0, 0, 0, 0);
        for (l, src) in &set {
            let spec = match (lang, *l) { (L::Rs, _) => engine::rust(), (L::Py, _) => engine::python(), (_, L::Tsx) => engine::tsx(), _ => engine::ts() };
            let mut p = engine::Pat::compile(&spec, pat).unwrap();
            p = match pred { 2 => p.with("E", not_ref), 3 => p.with_multi("ARGS", no_timeout_kw), 7 => p.with("MOD", relative_no_ext), _ => p };
            let (out, n, dropped) = engine::rewrite(&spec, &p, &ctx, src, tpl);
            if n > 0 {
                rw += n; files_changed += 1; reported += dropped;
                if engine::has_error(&spec.ts, &out) && !engine::has_error(&spec.ts, src) { new_err += 1; }
                second += engine::count(&spec, &p, &ctx, &out);
                comments_lost += count_comments(src).saturating_sub(count_comments(&out));
            }
        }
        println!("B {name:<20} rewrites={rw:<5} files={files_changed:<4} files-with-new-parse-errors={new_err:<3} matches-left-after-1-pass={second:<4} comments-lost={comments_lost} (self-reported {reported})");
    }
}

/// crude comment counter (line/inline comment markers)
fn count_comments(s: &str) -> usize {
    s.matches("//").count() + s.matches("/*").count() + s.matches('#').count()
}

// ---------------- throughput ----------------

#[derive(Clone, Copy, PartialEq)]
enum L { Rs, Py, Ts, Tsx }

fn files() -> Vec<(L, String)> {
    let mut v = vec![];
    for e in walkdir::WalkDir::new(CORPUS).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.components().any(|c| c.as_os_str() == "node_modules" || c.as_os_str() == ".git") {
            continue;
        }
        let l = match p.extension().and_then(|e| e.to_str()) {
            Some("rs") => L::Rs,
            Some("py") => L::Py,
            Some("ts") | Some("mts") | Some("cts") => L::Ts,
            Some("tsx") => L::Tsx,
            _ => continue,
        };
        if let Ok(s) = std::fs::read_to_string(p) {
            v.push((l, s));
        }
    }
    v
}

fn pats(l: L) -> &'static [&'static str] {
    match l {
        L::Rs => &["$X.unwrap()", "match $E { $$$ARMS }"],
        L::Py => &["httpx.get($$$ARGS)", "if $V is None:\n    $V = $D"],
        L::Ts | L::Tsx => &["$A == $B", "import { $$$NAMES } from '$MOD'"],
    }
}

fn bench() {
    let fs = files();
    let iters = 5;
    for lang in [L::Rs, L::Py, L::Ts] {
        let set: Vec<&String> = fs.iter().filter(|(l, _)| *l == lang || (lang == L::Ts && *l == L::Tsx)).map(|(_, s)| s).collect();
        let bytes: usize = set.iter().map(|s| s.len()).sum();
        let ctx = Ctx::new("bench");
        let name = match lang { L::Rs => "rust/ripgrep", L::Py => "python/httpx", _ => "ts/zod" };
        let tsx_flags: Vec<bool> = fs.iter().filter(|(l, _)| *l == lang || (lang == L::Ts && *l == L::Tsx)).map(|(l, _)| *l == L::Tsx).collect();

        // ---- A: ast-grep ----
        let (mut parse_t, mut match_t, mut hits) = (Duration::ZERO, Duration::ZERO, vec![0usize; 2]);
        for _ in 0..iters {
            hits = vec![0; 2];
            for (src, &is_tsx) in set.iter().zip(&tsx_flags) {
                macro_rules! go {
                    ($lg:expr) => {{
                        let rules: Vec<_> = pats(lang).iter().map(|p| agrep::Rule::new(p, $lg).unwrap()).collect();
                        let t = Instant::now();
                        let root = $lg.ast_grep(src.as_str());
                        parse_t += t.elapsed();
                        let t = Instant::now();
                        for (k, r) in rules.iter().enumerate() {
                            hits[k] += root.root().find_all(&r.bind(&ctx)).count();
                        }
                        match_t += t.elapsed();
                    }};
                }
                match (lang, is_tsx) { (L::Rs, _) => go!(Rust), (L::Py, _) => go!(Python), (_, false) => go!(TypeScript), (_, true) => go!(Tsx) }
            }
        }
        report("A ast-grep", name, set.len(), bytes, iters, parse_t, match_t, &hits);

        // ---- B: own engine ----
        let specs = [engine::rust(), engine::python(), engine::ts(), engine::tsx()];
        let (mut parse_t, mut match_t, mut hits) = (Duration::ZERO, Duration::ZERO, vec![0usize; 2]);
        let compiled: Vec<Vec<engine::Pat>> = specs.iter().enumerate().map(|(i, s)| {
            let l = [L::Rs, L::Py, L::Ts, L::Tsx][i];
            pats(l).iter().map(|p| engine::Pat::compile(s, p).unwrap()).collect()
        }).collect();
        let mut parser = tree_sitter::Parser::new();
        for _ in 0..iters {
            hits = vec![0; 2];
            for (src, &is_tsx) in set.iter().zip(&tsx_flags) {
                let si = match (lang, is_tsx) { (L::Rs, _) => 0, (L::Py, _) => 1, (_, false) => 2, (_, true) => 3 };
                parser.set_language(&specs[si].ts).unwrap();
                let t = Instant::now();
                let tree = parser.parse(src.as_str(), None).unwrap();
                parse_t += t.elapsed();
                let t = Instant::now();
                let mut st = engine::Stats { pred_calls: 0 };
                for (k, p) in compiled[si].iter().enumerate() {
                    hits[k] += engine::find_all(p, &ctx, &tree, src, &mut st).len();
                }
                match_t += t.elapsed();
            }
        }
        report("B own", name, set.len(), bytes, iters, parse_t, match_t, &hits);
    }
}

#[allow(clippy::too_many_arguments)]
fn report(eng: &str, name: &str, n: usize, bytes: usize, iters: u32, p: Duration, m: Duration, hits: &[usize]) {
    let per = |d: Duration| d.as_secs_f64() / iters as f64;
    println!(
        "{eng:<11} {name:<13} files={n:<4} {:.1}MB  parse={:>7.1}ms  match(2 pats)={:>7.1}ms  => {:>6.0} files/s total, {:>6.0} files/s match-only; hits={hits:?}",
        bytes as f64 / 1e6,
        per(p) * 1e3,
        per(m) * 1e3,
        n as f64 / (per(p) + per(m)),
        n as f64 / per(m),
    );
}

#[cfg(test)]
mod sanity {
    use ast_grep_core::matcher::MatcherExt;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_core::Pattern;
    use ast_grep_language::{Python, TypeScript};
    #[test]
    fn plain_pattern_last_arg() {
        for (p, s) in [("f($$$A, $B)", "f(1, 2)"), ("f($$$A, $B)", "f(1, 2, 3)"), ("f($$$A, $B)", "f(a, b, c)")] {
            let pat = Pattern::new(p, TypeScript);
            let root = TypeScript.ast_grep(s);
            let n = root.root().find_all(&pat).count();
            let m = pat.find_node(root.root()).map(|m| m.get_env().get_match("B").map(|b| b.text().to_string()));
            println!("TS {p} on {s}: {n} {m:?}");
        }
        let pat = Pattern::new("foo($$$A, timeout=$T)", Python);
        let root = Python.ast_grep("foo(a, b, timeout=3)\nfoo(timeout=3, b, timeout=4)");
        println!("PY: {}", root.root().find_all(&pat).count());
    }
}

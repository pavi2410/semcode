//! Option B: a minimal own SSR engine over tree-sitter.
//!
//! * Pattern text: `$X` -> `__ssr_X`, `$$$XS` -> `__ssrm_XS` (valid identifiers in Rust/Python/TS),
//!   parsed by the *same* grammar, optionally inside context wrappers (e.g. `fn f(){ ... }`).
//! * Unification over non-extra children (comments ignored), anonymous tokens must match
//!   (so `==` != `===`), `$$$` is matched with full backtracking.
//! * Predicates are evaluated *at bind time inside the backtracking*, so a failing semantic
//!   check makes `$$$` try a different split.
//! * Replacement: template with indentation rebasing, list-aware comma elision for empty `$$$`,
//!   and nested rewrites inside bindings (e.g. `a.unwrap().b.unwrap()` in one pass).

use crate::view::{Ctx, NodeView};
use regex::Regex;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, OnceLock};
use tree_sitter::{Language, Node, Parser, Tree};

const PH: &str = "__ssr_";
const PHM: &str = "__ssrm_";

#[derive(Debug, Clone)]
pub enum P {
    Var(String),
    Multi(String),
    Leaf { kind: u16, text: String },
    Inner { kind: u16, children: Vec<P> },
}

pub type SinglePred = Arc<dyn Fn(&Ctx, &NodeView) -> bool + Send + Sync>;
pub type MultiPred = Arc<dyn Fn(&Ctx, &[NodeView]) -> bool + Send + Sync>;

pub struct Pat {
    pub root: P,
    pub single: HashMap<String, SinglePred>,
    pub multi: HashMap<String, MultiPred>,
    /// which wrapper context was needed to parse the fragment (diagnostics)
    pub context_used: usize,
}

pub struct LangSpec {
    pub ts: Language,
    /// (prefix, suffix) wrappers tried in order when the bare fragment does not parse.
    pub contexts: Vec<(&'static str, &'static str)>,
}

pub fn rust() -> LangSpec {
    LangSpec {
        ts: tree_sitter_rust::LANGUAGE.into(),
        contexts: vec![("", ""), ("fn __ssr_ctx() { ", " }"), ("fn __ssr_ctx() { ", "; }")],
    }
}
pub fn python() -> LangSpec {
    LangSpec { ts: tree_sitter_python::LANGUAGE.into(), contexts: vec![("", "")] }
}
pub fn ts() -> LangSpec {
    LangSpec { ts: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), contexts: vec![("", "")] }
}
pub fn tsx() -> LangSpec {
    LangSpec { ts: tree_sitter_typescript::LANGUAGE_TSX.into(), contexts: vec![("", "")] }
}

fn mv_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\$\$\$([A-Za-z_][A-Za-z0-9_]*)|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap())
}

fn preprocess(p: &str) -> String {
    mv_re()
        .replace_all(p, |c: &regex::Captures| match (c.get(1), c.get(2)) {
            (Some(m), _) => format!("{PHM}{}", m.as_str()),
            (_, Some(s)) => format!("{PH}{}", s.as_str()),
            _ => unreachable!(),
        })
        .into_owned()
}

fn placeholder(text: &str) -> Option<P> {
    if let Some(n) = text.strip_prefix(PHM) {
        if n.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some(P::Multi(n.into()));
        }
    }
    if let Some(n) = text.strip_prefix(PH) {
        if n.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some(P::Var(n.into()));
        }
    }
    None
}

pub fn parse(lang: &Language, src: &str) -> Tree {
    let mut p = Parser::new();
    p.set_language(lang).unwrap();
    p.parse(src, None).unwrap()
}

/// Non-extra children (comments etc. are skipped on both sides).
fn sig_children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.children(&mut c).filter(|k| (k.is_error() || !k.is_extra()) && !k.is_missing()).collect()
}

fn text<'s>(n: Node, src: &'s str) -> &'s str {
    &src[n.byte_range()]
}

fn to_p(n: Node, src: &str) -> P {
    if let Some(p) = placeholder(text(n, src).trim()) {
        return p;
    }
    let ch = sig_children(n);
    if ch.is_empty() {
        P::Leaf { kind: n.kind_id(), text: text(n, src).to_string() }
    } else {
        P::Inner { kind: n.kind_id(), children: ch.into_iter().map(|c| to_p(c, src)).collect() }
    }
}

/// Error nodes are tolerated only if they consist solely of a placeholder.
fn bad_errors(n: Node, src: &str) -> bool {
    if (n.is_error() || n.is_missing()) && placeholder(text(n, src).trim()).is_none() {
        return true;
    }
    let mut c = n.walk();
    let kids: Vec<_> = n.children(&mut c).collect();
    kids.into_iter().any(|k| bad_errors(k, src))
}

/// Innermost node covering exactly `span`.
fn node_for_span(root: Node, span: Range<usize>) -> Option<Node> {
    let mut cur = root;
    let mut best = None;
    loop {
        if cur.byte_range() == span {
            best = Some(cur);
        }
        let mut c = cur.walk();
        let next = cur
            .children(&mut c)
            .find(|k| k.start_byte() <= span.start && k.end_byte() >= span.end && !k.is_extra());
        match next {
            Some(k) => cur = k,
            None => return best,
        }
    }
}

impl Pat {
    pub fn compile(lang: &LangSpec, pattern: &str) -> Result<Pat, String> {
        let body = preprocess(pattern.trim());
        for (i, (pre, suf)) in lang.contexts.iter().enumerate() {
            let src = format!("{pre}{body}{suf}");
            let tree = parse(&lang.ts, &src);
            if bad_errors(tree.root_node(), &src) {
                continue;
            }
            let span = pre.len()..pre.len() + body.len();
            if let Some(n) = node_for_span(tree.root_node(), span) {
                return Ok(Pat { root: to_p(n, &src), single: HashMap::new(), multi: HashMap::new(), context_used: i });
            }
        }
        Err(format!("pattern does not parse as a single node: {pattern}"))
    }
    pub fn with(mut self, var: &str, p: impl Fn(&Ctx, &NodeView) -> bool + Send + Sync + 'static) -> Self {
        self.single.insert(var.into(), Arc::new(p));
        self
    }
    pub fn with_multi(mut self, var: &str, p: impl Fn(&Ctx, &[NodeView]) -> bool + Send + Sync + 'static) -> Self {
        self.multi.insert(var.into(), Arc::new(p));
        self
    }
    fn root_kind(&self) -> Option<u16> {
        match &self.root {
            P::Leaf { kind, .. } | P::Inner { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Bind<'t> {
    One(Node<'t>),
    Many(Vec<Node<'t>>),
}

#[derive(Debug, Clone)]
pub struct Match<'t> {
    pub node: Node<'t>,
    /// replaced range (node range minus an unmatched trailing `;`)
    pub range: Range<usize>,
    pub env: Vec<(String, Bind<'t>)>,
}

struct M<'a, 't> {
    pat: &'a Pat,
    ctx: &'a Ctx,
    src: &'t str,
    pub pred_calls: usize,
}

fn nv(n: Node, src: &str) -> NodeView {
    NodeView { kind: n.kind().into(), text: text(n, src).into(), range: n.byte_range() }
}

/// Structural equality ignoring whitespace/comments (for repeated metavariables).
fn same(a: Node, b: Node, src: &str) -> bool {
    if a.kind_id() != b.kind_id() {
        return false;
    }
    let (ca, cb) = (sig_children(a), sig_children(b));
    if ca.is_empty() && cb.is_empty() {
        return text(a, src) == text(b, src);
    }
    ca.len() == cb.len() && ca.iter().zip(cb.iter()).all(|(x, y)| same(*x, *y, src))
}

impl<'a, 't> M<'a, 't> {
    fn bind_one(&mut self, name: &str, n: Node<'t>, env: &mut Vec<(String, Bind<'t>)>) -> bool {
        if let Some((_, b)) = env.iter().find(|(k, _)| k == name) {
            return matches!(b, Bind::One(o) if same(*o, n, self.src));
        }
        if let Some(p) = self.pat.single.get(name) {
            self.pred_calls += 1;
            if !p(self.ctx, &nv(n, self.src)) {
                return false;
            }
        }
        env.push((name.into(), Bind::One(n)));
        true
    }
    fn bind_many(&mut self, name: &str, ns: &[Node<'t>], env: &mut Vec<(String, Bind<'t>)>) -> bool {
        if let Some((_, b)) = env.iter().find(|(k, _)| k == name) {
            return matches!(b, Bind::Many(o) if o.len() == ns.len() && o.iter().zip(ns).all(|(x, y)| same(*x, *y, self.src)));
        }
        if let Some(p) = self.pat.multi.get(name) {
            self.pred_calls += 1;
            let views: Vec<_> = ns.iter().filter(|n| n.is_named()).map(|n| nv(*n, self.src)).collect();
            if !p(self.ctx, &views) {
                return false;
            }
        }
        env.push((name.into(), Bind::Many(ns.to_vec())));
        true
    }
    fn node(&mut self, p: &P, n: Node<'t>, env: &mut Vec<(String, Bind<'t>)>) -> bool {
        match p {
            P::Var(name) => n.is_named() && self.bind_one(name, n, env),
            P::Multi(name) => self.bind_many(name, &[n], env),
            P::Leaf { kind, text: t } => {
                let nt = text(n, self.src);
                // quote style is not significant: pattern '$MOD' matches "zod"
                let q = |s: &str| s == "'" || s == "\"";
                (n.kind_id() == *kind && nt == t) || (q(nt) && q(t))
            }
            P::Inner { kind, children } => {
                n.kind_id() == *kind && {
                    let cs = sig_children(n);
                    self.seq(children, &cs, env)
                }
            }
        }
    }
    fn seq(&mut self, ps: &[P], cs: &[Node<'t>], env: &mut Vec<(String, Bind<'t>)>) -> bool {
        // trailing statement terminators in the target are optional (`import ... from 'm'` vs `...;`)
        let Some(first) = ps.first() else { return cs.iter().all(|c| !c.is_named() && text(*c, self.src) == ";") };
        let mark = env.len();
        let is_sep = |p: Option<&P>| matches!(p, Some(P::Leaf { text, .. }) if text == "," || text == ";");
        // list separators adjacent to an EMPTY $$$ are optional: `f($$$A, $B)` matches `f(1)`
        if is_sep(Some(first)) {
            if let Some(P::Multi(name)) = ps.get(1) {
                if self.bind_many(name, &[], env) && self.seq(&ps[2..], cs, env) {
                    return true;
                }
                env.truncate(mark);
            }
        }
        if let (P::Multi(name), true) = (first, is_sep(ps.get(1))) {
            if self.bind_many(name, &[], env) && self.seq(&ps[2..], cs, env) {
                return true;
            }
            env.truncate(mark);
        }
        if let P::Multi(name) = first {
            // lazy: shortest binding first; full backtracking
            for k in 0..=cs.len() {
                if self.bind_many(name, &cs[..k], env) && self.seq(&ps[1..], &cs[k..], env) {
                    return true;
                }
                env.truncate(mark);
            }
            return false;
        }
        if cs.is_empty() {
            return false;
        }
        if self.node(first, cs[0], env) && self.seq(&ps[1..], &cs[1..], env) {
            return true;
        }
        env.truncate(mark);
        false
    }
}

pub struct Stats {
    pub pred_calls: usize,
}

/// All matches (reentrant: nested matches included), in document order.
pub fn find_all<'t>(pat: &Pat, ctx: &Ctx, tree: &'t Tree, src: &'t str, stats: &mut Stats) -> Vec<Match<'t>> {
    let mut m = M { pat, ctx, src, pred_calls: 0 };
    let rk = pat.root_kind();
    let mut out = vec![];
    let mut cur = tree.walk();
    'outer: loop {
        let n = cur.node();
        if rk.map_or(n.is_named(), |k| k == n.kind_id()) {
            let mut env = vec![];
            if m.node(&pat.root, n, &mut env) {
                let mut range = n.byte_range();
                let pat_ends_semi = matches!(&pat.root, P::Inner { children, .. } if matches!(children.last(), Some(P::Leaf { text, .. }) if text == ";"));
                if let Some(last) = sig_children(n).last() {
                    if !pat_ends_semi && !last.is_named() && text(*last, src) == ";" {
                        range.end = src[..last.start_byte()].trim_end().len();
                    }
                }
                out.push(Match { node: n, range, env });
            }
        }
        if cur.goto_first_child() {
            continue;
        }
        loop {
            if cur.goto_next_sibling() {
                continue 'outer;
            }
            if !cur.goto_parent() {
                break 'outer;
            }
        }
    }
    stats.pred_calls += m.pred_calls;
    out
}

// ---------------- replacement ----------------

enum Frag {
    Text(String),
    Var(String),
    Multi(String),
}

fn parse_template(t: &str) -> Vec<Frag> {
    let mut out = vec![];
    let mut last = 0;
    for c in mv_re().captures_iter(t) {
        let m = c.get(0).unwrap();
        out.push(Frag::Text(t[last..m.start()].into()));
        out.push(match (c.get(1), c.get(2)) {
            (Some(x), _) => Frag::Multi(x.as_str().into()),
            (_, Some(x)) => Frag::Var(x.as_str().into()),
            _ => unreachable!(),
        });
        last = m.end();
    }
    out.push(Frag::Text(t[last..].into()));
    out
}

fn line_indent(src: &str, at: usize) -> &str {
    let ls = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let rest = &src[ls..];
    let w = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    &rest[..w.min(at - ls)]
}

/// Re-base continuation lines of `s` from `old` indentation to `new`.
fn reindent(s: &str, old: &str, new: &str) -> String {
    let mut out = String::new();
    for (i, l) in s.split('\n').enumerate() {
        if i == 0 {
            out.push_str(l);
            continue;
        }
        out.push('\n');
        if l.trim().is_empty() {
            continue;
        }
        out.push_str(new);
        out.push_str(l.strip_prefix(old).unwrap_or(l.trim_start()));
    }
    out
}

pub struct Rewriter<'a, 't> {
    pub src: &'t str,
    pub matches: &'a [Match<'t>],
    template: Vec<Frag>,
}

impl<'a, 't> Rewriter<'a, 't> {
    pub fn new(src: &'t str, matches: &'a [Match<'t>], template: &str) -> Self {
        Rewriter { src, matches, template: parse_template(template) }
    }

    /// Source text of `range` with all outermost matches inside it rewritten.
    pub fn rewrite_range(&self, range: Range<usize>, map: Option<&mut Vec<(Range<usize>, Range<usize>)>>) -> (String, usize) {
        let mut out = String::new();
        let mut pos = range.start;
        let mut n = 0;
        let mut map = map;
        for (i, m) in self.matches.iter().enumerate() {
            let r = m.range.clone();
            if r.start < pos || r.end > range.end || r.start < range.start {
                continue;
            }
            out.push_str(&self.src[pos..r.start]);
            let (s, k) = self.render(i);
            if let Some(mp) = map.as_deref_mut() {
                mp.push((r.clone(), out.len()..out.len() + s.len()));
            }
            out.push_str(&s);
            n += 1 + k;
            pos = r.end;
        }
        out.push_str(&self.src[pos..range.end]);
        (out, n)
    }

    fn binding_text(&self, b: &Bind<'t>) -> Option<(String, usize, usize)> {
        let r = match b {
            Bind::One(n) => n.byte_range(),
            Bind::Many(ns) => {
                // trim leading/trailing anonymous tokens (e.g. trailing comma)
                let named: Vec<_> = ns.iter().filter(|n| n.is_named()).collect();
                let (f, l) = (named.first()?, named.last()?);
                f.start_byte()..l.end_byte()
            }
        };
        let (s, n) = self.rewrite_range(r.clone(), None);
        Some((s, r.start, n))
    }

    /// Render match `i` (returns text, number of nested rewrites).
    pub fn render(&self, i: usize) -> (String, usize) {
        let m = &self.matches[i];
        let get = |name: &str| m.env.iter().find(|(k, _)| k == name).map(|(_, b)| b);
        let mut out = String::new();
        let mut nested = 0;
        let mut skip_comma = false;
        for (fi, f) in self.template.iter().enumerate() {
            match f {
                Frag::Text(t) => {
                    let t = if skip_comma { t.trim_start_matches(|c| c == ',' || c == ' ').to_string() } else { t.clone() };
                    skip_comma = false;
                    out.push_str(&t);
                }
                Frag::Var(v) | Frag::Multi(v) => {
                    let cur_line_indent = {
                        let ls = out.rfind('\n').map_or(0, |i| i + 1);
                        let l = &out[ls..];
                        l[..l.len() - l.trim_start_matches([' ', '\t']).len()].to_string()
                    };
                    match get(v).and_then(|b| self.binding_text(b)) {
                        Some((s, start, n)) => {
                            nested += n;
                            out.push_str(&reindent(&s, line_indent(self.src, start), &cur_line_indent));
                        }
                        None => {
                            // empty $$$: elide an adjacent list separator
                            let next_is_comma = matches!(self.template.get(fi + 1), Some(Frag::Text(t)) if t.starts_with(','));
                            if next_is_comma {
                                skip_comma = true;
                            } else if out.trim_end().ends_with(',') {
                                let k = out.trim_end().len() - 1;
                                out.truncate(k);
                            }
                        }
                    }
                }
            }
        }
        // template lines after the first get the match's indentation
        let base = line_indent(self.src, m.range.start);
        let out = reindent(&out, "", base);
        (out, nested)
    }

    /// Whole-file rewrite. Returns (new text, number of rewritten matches incl. nested).
    pub fn apply(&self) -> (String, usize) {
        self.rewrite_range(0..self.src.len(), None)
    }

    /// Like `apply`, but afterwards splices comments from the original back into the
    /// rendered text wherever the surrounding tokens survived (token-level LCS).
    /// Returns (text, rewrites, comments_dropped).
    pub fn apply_preserving(&self, lang: &Language, orig: &Tree) -> (String, usize, usize) {
        let mut map = vec![];
        let (out, n) = self.rewrite_range(0..self.src.len(), Some(&mut map));
        let new_tree = parse(lang, &out);
        let mut res = out.clone();
        let mut dropped = 0;
        for (old_r, new_r) in map.iter().rev() {
            let (txt, d) = preserve_trivia(self.src, orig, old_r.clone(), &out, &new_tree, new_r.clone());
            dropped += d;
            res.replace_range(new_r.clone(), &txt);
        }
        (res, n, dropped)
    }
}

pub fn has_error(lang: &Language, src: &str) -> bool {
    parse(lang, src).root_node().has_error()
}

/// One-shot helper: (new_src, n_rewrites, pred_calls)
pub fn rewrite(lang: &LangSpec, pat: &Pat, ctx: &Ctx, src: &str, template: &str) -> (String, usize, usize) {
    let tree = parse(&lang.ts, src);
    let mut st = Stats { pred_calls: 0 };
    let ms = find_all(pat, ctx, &tree, src, &mut st);
    Rewriter::new(src, &ms, template).apply_preserving(&lang.ts, &tree)
}

pub fn rewrite_plain(lang: &LangSpec, pat: &Pat, ctx: &Ctx, src: &str, template: &str) -> (String, usize) {
    let tree = parse(&lang.ts, src);
    let mut st = Stats { pred_calls: 0 };
    let ms = find_all(pat, ctx, &tree, src, &mut st);
    Rewriter::new(src, &ms, template).apply()
}

// ---------------- trivia (comment) preservation ----------------

/// Non-extra leaf tokens fully inside `r`.
fn leaves_in(tree: &Tree, r: &Range<usize>) -> Vec<Range<usize>> {
    fn go(n: Node, r: &Range<usize>, out: &mut Vec<Range<usize>>) {
        if n.end_byte() <= r.start || n.start_byte() >= r.end || n.is_extra() {
            return;
        }
        if n.child_count() == 0 {
            if n.start_byte() >= r.start && n.end_byte() <= r.end && n.start_byte() < n.end_byte() {
                out.push(n.byte_range());
            }
            return;
        }
        let mut c = n.walk();
        for k in n.children(&mut c) {
            go(k, r, out);
        }
    }
    let mut out = vec![];
    go(tree.root_node(), r, &mut out);
    out
}

/// Re-base indentation inside a gap (whitespace/comments between tokens). Unlike `reindent`,
/// the trailing whitespace-only line is the indentation of the next token and is kept.
fn rebase_gap(seg: &str, old: &str, new: &str) -> String {
    // relative re-indent: keep each line's offset w.r.t. the anchor line's indentation
    let mut out = String::new();
    for (i, l) in seg.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            let ws = l.len() - l.trim_start_matches(' ').len();
            if old.contains('\t') || new.contains('\t') {
                out.push_str(l);
                continue;
            }
            let rel = ws as isize - old.len() as isize;
            let target = (new.len() as isize + rel).max(0) as usize;
            out.push_str(&" ".repeat(target));
            out.push_str(&l[ws..]);
        } else {
            out.push_str(l);
        }
    }
    out
}

/// Minimal-edit pass ("edit script"). Token-level LCS between the original match tokens
/// and the rendered tokens. Between two surviving tokens:
///   * no token changes  -> keep the ORIGINAL gap (whitespace + comments), indentation re-based;
///   * only insertions   -> new tokens go right after the left anchor, original gap (with any
///                          comment) follows: `x,  # c` + `timeout=10` => `x, timeout=10  # c`;
///   * deletions         -> take the rendered segment; comments that were there are reported.
/// Returns (text, comments_dropped).
fn preserve_trivia(osrc: &str, otree: &Tree, or: Range<usize>, nsrc: &str, ntree: &Tree, nr: Range<usize>) -> (String, usize) {
    let a = leaves_in(otree, &or);
    let b = leaves_in(ntree, &nr);
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if osrc[a[i].clone()] == nsrc[b[j].clone()] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut pairs = vec![];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if osrc[a[i].clone()] == nsrc[b[j].clone()] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let line_prefix = {
        let ls = nsrc[..nr.start].rfind('\n').map_or(0, |k| k + 1);
        nsrc[ls..nr.start].to_string()
    };
    let cur_indent = |out: &str| -> String {
        let full = format!("{line_prefix}{out}");
        let l = &full[full.rfind('\n').map_or(0, |k| k + 1)..];
        l[..l.len() - l.trim_start_matches([' ', '\t']).len()].to_string()
    };
    let mut out = String::new();
    let mut dropped = 0;
    let mut prev: Option<(usize, usize)> = None;
    for k in 0..=pairs.len() {
        let (o_end, n_end) = match pairs.get(k) {
            Some(&(oi, ni)) => (a[oi].start, b[ni].start),
            None => (or.end, nr.end),
        };
        let o_start = prev.map_or(or.start, |(oi, _)| a[oi].end);
        let n_start = prev.map_or(nr.start, |(_, ni)| b[ni].end);
        let o_seg = &osrc[o_start..o_end];
        let n_seg = &nsrc[n_start..n_end];
        let o_has_tok = a.iter().any(|t| t.start >= o_start && t.end <= o_end);
        let n_tok_end = b.iter().filter(|t| t.start >= n_start && t.end <= n_end).map(|t| t.end).max();
        let rebase = |seg: &str, out: &str| -> String {
            let old_ind = line_indent(osrc, o_start).to_string();
            rebase_gap(seg, &old_ind, &cur_indent(out))
        };
        match (o_has_tok, n_tok_end) {
            (false, None) => {
                // unchanged tokens around: keep original trivia (unless at the virtual ends)
                if prev.is_some() && pairs.get(k).is_some() {
                    let s = rebase(o_seg, &out);
                    out.push_str(&s);
                } else {
                    out.push_str(n_seg);
                }
            }
            (false, Some(e)) => {
                out.push_str(&nsrc[n_start..e]);
                if prev.is_some() && pairs.get(k).is_some() && (!o_seg.trim().is_empty() || o_seg.contains('\n')) {
                    let s = rebase(o_seg, &out);
                    out.push_str(&s);
                } else {
                    out.push_str(&nsrc[e..n_end]);
                }
            }
            (true, _) => {
                let mut comments = String::new();
                let mut last = o_start;
                for t in a.iter().filter(|t| t.start >= o_start && t.end <= o_end) {
                    comments.push_str(&osrc[last..t.start]);
                    last = t.end;
                }
                comments.push_str(&osrc[last..o_end]);
                dropped += comments.lines().filter(|l| !l.trim().is_empty() && !n_seg.contains(l.trim())).count();
                out.push_str(n_seg);
            }
        }
        if let Some(&(oi, ni)) = pairs.get(k) {
            out.push_str(&nsrc[b[ni].clone()]);
            prev = Some((oi, ni));
        }
    }
    (out, dropped)
}


pub fn count(lang: &LangSpec, pat: &Pat, ctx: &Ctx, src: &str) -> usize {
    let tree = parse(&lang.ts, src);
    let mut st = Stats { pred_calls: 0 };
    find_all(pat, ctx, &tree, src, &mut st).len()
}

pub fn debug_sexp(lang: &LangSpec, pattern: &str) -> String {
    let body = preprocess(pattern.trim());
    let mut s = String::new();
    for (pre, suf) in &lang.contexts {
        let src = format!("{pre}{body}{suf}");
        let t = parse(&lang.ts, &src);
        s += &format!("ctx[{pre}|{suf}] bad_errors={} {}\n", bad_errors(t.root_node(), &src), t.root_node().to_sexp());
    }
    s
}

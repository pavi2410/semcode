//! Option A: ast-grep-core 0.50 used as a library, with a semantic-predicate hook.
//!
//! `Matcher::match_node_with_env` is generic over `D: Doc`, so a user closure cannot
//! be a Matcher directly (ast-grep's own `Op::func`/`Predicate` is commented out in 0.50).
//! We bridge by extracting a non-generic `NodeView` (kind/text/range) and calling a
//! boxed `dyn Fn`. Constraints run *after* the structural match completes (post-hoc),
//! exactly like ast-grep-config's `constraints:` – they cannot steer `$$$` splits.

use ast_grep_core::matcher::Matcher;
use ast_grep_core::meta_var::MetaVarEnv;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc};
use ast_grep_core::{Doc, Node, Pattern};
use bit_set::BitSet;
use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

pub use crate::view::{Ctx, NodeView};

pub type SinglePred = Arc<dyn Fn(&Ctx, &NodeView) -> bool + Send + Sync>;
pub type MultiPred = Arc<dyn Fn(&Ctx, &[NodeView]) -> bool + Send + Sync>;

pub struct Rule {
    pub pattern: Pattern,
    pub single: Vec<(String, SinglePred)>,
    pub multi: Vec<(String, MultiPred)>,
}

impl Rule {
    pub fn new<L: LanguageExt>(pat: &str, lang: L) -> Result<Self, String> {
        let pattern = Pattern::try_new(pat, lang).map_err(|e| e.to_string())?;
        Ok(Rule { pattern, single: vec![], multi: vec![] })
    }
    pub fn with(mut self, var: &str, p: impl Fn(&Ctx, &NodeView) -> bool + Send + Sync + 'static) -> Self {
        self.single.push((var.into(), Arc::new(p)));
        self
    }
    pub fn with_multi(mut self, var: &str, p: impl Fn(&Ctx, &[NodeView]) -> bool + Send + Sync + 'static) -> Self {
        self.multi.push((var.into(), Arc::new(p)));
        self
    }
    pub fn bind<'a>(&'a self, ctx: &'a Ctx) -> Bound<'a> {
        Bound { rule: self, ctx }
    }
}

/// A rule bound to a file context (path etc.), so predicates can ask the semantic layer.
pub struct Bound<'a> {
    rule: &'a Rule,
    ctx: &'a Ctx,
}

fn view<D: Doc>(n: &Node<'_, D>) -> NodeView {
    let r: Range<usize> = n.range();
    NodeView { kind: n.kind().to_string(), text: n.text().to_string(), range: r }
}

impl Matcher for Bound<'_> {
    fn match_node_with_env<'tree, D: Doc>(
        &self,
        node: Node<'tree, D>,
        env: &mut Cow<MetaVarEnv<'tree, D>>,
    ) -> Option<Node<'tree, D>> {
        let n = self.rule.pattern.match_node_with_env(node, env)?;
        for (var, p) in &self.rule.single {
            let m = env.get_match(var)?;
            if !p(self.ctx, &view(m)) {
                return None;
            }
        }
        for (var, p) in &self.rule.multi {
            let ms: Vec<NodeView> = env.get_multiple_matches(var).iter().map(view).collect();
            if !p(self.ctx, &ms) {
                return None;
            }
        }
        Some(n)
    }
    fn potential_kinds(&self) -> Option<BitSet> {
        self.rule.pattern.potential_kinds()
    }
    fn get_match_len<D: Doc>(&self, node: Node<'_, D>) -> Option<usize> {
        self.rule.pattern.get_match_len(node)
    }
}

/// Count matches in `src`.
pub fn count<L: LanguageExt>(rule: &Rule, ctx: &Ctx, src: &str, lang: L) -> usize {
    let root = lang.ast_grep(src);
    let b = rule.bind(ctx);
    root.root().find_all(&b).count()
}

/// Rewrite all (non-overlapping, outermost) matches with a template. Returns (new_src, n_edits).
pub fn rewrite<L: LanguageExt>(rule: &Rule, ctx: &Ctx, src: &str, lang: L, template: &str) -> (String, usize) {
    let root = lang.ast_grep(src);
    let b = rule.bind(ctx);
    let edits = root.root().replace_all(&b, template);
    let n = edits.len();
    let mut out = src.as_bytes().to_vec();
    for e in edits.iter().rev() {
        out.splice(e.position..e.position + e.deleted_length, e.inserted_text.iter().copied());
    }
    (String::from_utf8(out).unwrap(), n)
}

pub fn root_has_error<L: LanguageExt>(src: &str, lang: L) -> bool {
    let root: ast_grep_core::AstGrep<StrDoc<L>> = lang.ast_grep(src);
    root.root().get_inner_node().has_error()
}

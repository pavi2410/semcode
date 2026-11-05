//! Engine-neutral types handed to semantic predicates. This is the shape of the
//! "hook" the semantic layer would implement: given file + byte range (+ kind/text
//! for cheap syntactic checks), answer type/resolution questions.
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct NodeView {
    pub kind: String,
    pub text: String,
    pub range: Range<usize>,
}

/// Per-file context. In semcode this would carry a handle to the semantic DB.
pub struct Ctx {
    pub path: String,
    /// Fake semantic oracle: (path, byte range) -> "type name". Stands in for a type checker.
    pub types: std::collections::HashMap<(String, usize), String>,
}

impl Ctx {
    pub fn new(path: &str) -> Self {
        Ctx { path: path.into(), types: Default::default() }
    }
    /// Simulated semantic query: "what's the type of the expression starting at byte `start`".
    pub fn type_of(&self, v: &NodeView) -> Option<&str> {
        self.types.get(&(self.path.clone(), v.range.start)).map(|s| s.as_str())
    }
}

//! Option C: tree-sitter queries as an escape hatch. Built-in #eq?/#match? are evaluated by
//! tree-sitter; unknown predicates (here `#sem-type? @x "io::Result"`) are surfaced via
//! `Query::general_predicates()` and evaluated by us -> a real hook for the semantic layer.
use crate::engine::parse;
use crate::view::{Ctx, NodeView};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor, QueryPredicateArg};

pub fn demo() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/sample.rs")).unwrap();
    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let tree = parse(&lang, &src);
    let mut ctx = Ctx::new("sample.rs");
    for (i, _) in src.match_indices("File::open") {
        ctx.types.insert(("sample.rs".into(), i), "io::Result<File>".into());
    }
    // `$X.unwrap()` where type_of($X) is io::Result
    let q = Query::new(
        &lang,
        r#"((call_expression
             function: (field_expression value: (_) @x field: (field_identifier) @m)
             arguments: (arguments . ")"))
            (#eq? @m "unwrap")
            (#sem-type? @x "io::Result")) @call"#,
    )
    .unwrap();
    let mut cur = QueryCursor::new();
    let mut it = cur.matches(&q, tree.root_node(), src.as_bytes());
    let (mut syntactic, mut semantic) = (0, 0);
    while let Some(m) = it.next() {
        syntactic += 1;
        let ok = q.general_predicates(m.pattern_index).iter().all(|p| {
            if p.operator.as_ref() != "sem-type?" {
                return true;
            }
            let (QueryPredicateArg::Capture(ci), QueryPredicateArg::String(want)) = (&p.args[0], &p.args[1]) else { return false };
            m.captures().iter().filter(|c| c.index == *ci).all(|c| {
                let v = NodeView { kind: c.node.kind().into(), text: c.node.utf8_text(src.as_bytes()).unwrap().into(), range: c.node.byte_range() };
                ctx.type_of(&v).is_some_and(|t| t.starts_with(want.as_ref()))
            })
        });
        if ok {
            semantic += 1;
            let call = m.captures().iter().find(|c| q.capture_names()[c.index as usize] == "call").unwrap();
            println!("semantic match: {}", call.node.utf8_text(src.as_bytes()).unwrap());
        }
    }
    println!("query: {syntactic} syntactic unwrap() matches, {semantic} after custom #sem-type? predicate");
}

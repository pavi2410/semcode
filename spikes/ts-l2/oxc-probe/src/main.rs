//! oxc tier probe: parse + semantic over every .ts file; per-file scopes/symbols/refs + module record.
use oxc::span::GetSpan;
use oxc::{allocator::Allocator, parser::Parser, semantic::SemanticBuilder, span::SourceType};
use std::{path::Path, time::Instant};

fn walk(p: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(p).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "ts") { out.push(p) }
    }
}
fn main() {
    let root = std::env::args().nth(1).unwrap();
    let mut files = vec![]; walk(Path::new(&root), &mut files);
    let t = Instant::now();
    let (mut syms, mut refs, mut unresolved, mut imports, mut errs, mut bytes) = (0, 0, 0, 0, 0, 0);
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap(); bytes += src.len();
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, &src, SourceType::from_path(f).unwrap()).parse();
        errs += ret.diagnostics.len();
        imports += ret.module_record.import_entries.len();
        let sem = SemanticBuilder::new().with_build_nodes(true).build(&ret.program).semantic;
        let sc = sem.scoping();
        syms += sc.symbols_len();
        refs += sc.symbol_ids().map(|s| sc.get_resolved_reference_ids(s).len()).sum::<usize>();
        unresolved += sc.root_unresolved_references().values().map(|v| v.len()).sum::<usize>();
        if f.ends_with("alias_probe.ts") {
            println!("--- alias_probe.ts symbols (name: resolved refs @ byte offsets)");
            for s in sc.symbol_ids() {
                let offs: Vec<u32> = sc.get_resolved_reference_ids(s).iter().map(|r| sem.nodes().get_node(sc.get_reference(*r).node_id()).span().start).collect();
                println!("  {}: {} {:?}", sc.symbol_name(s), offs.len(), offs);
            }
            for ie in &ret.module_record.import_entries {
                println!("  import {:?} as {} from {:?}", ie.import_name, ie.local_name.name, ret.module_record.requested_modules.keys().collect::<Vec<_>>());
            }
            println!("  unresolved: {:?}", sc.root_unresolved_references().keys().collect::<Vec<_>>());
        }
    }
    println!("files={} MB={:.1} time={:?} symbols={} resolved_refs={} unresolved_refs={} import_entries={} parse_errors={}",
        files.len(), bytes as f64 / 1e6, t.elapsed(), syms, refs, unresolved, imports, errs);
}

//! Option A spike: embed rust-analyzer via ra_ap_* crates (pinned =0.0.358).
//! Workspace loading mirrors rust-analyzer's own `analysis-stats`/`scip` CLI
//! (crates/rust-analyzer/src/cli/{analysis_stats,scip}.rs @ 2ad81e72).
//!
//! usage: ra-lib <ripgrep-root> [--all-features] [--apply-rename]
use std::{path::Path, time::Instant};

use ra_ap_hir::ChangeWithProcMacros;
use ra_ap_ide::{
    AnalysisHost, CallHierarchyConfig, FileId, FilePosition, FileRange, FindAllRefsConfig,
    GotoDefinitionConfig, HoverConfig, HoverDocFormat, LineCol, RaFixtureConfig, RenameConfig,
    SubstTyLen, TextRange, TextSize,
};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{CargoConfig, CargoFeatures, RustLibSource};
use ra_ap_vfs::{Vfs, VfsPath};

/// (RSS, HWM) in MB of this process plus its children (proc-macro-srv).
fn rss_mb() -> (u64, u64) {
    let me = std::process::id().to_string();
    let kids = std::process::Command::new("pgrep").args(["-P", &me]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let (mut rss, mut hwm) = (0, 0);
    for pid in std::iter::once(me.as_str()).chain(kids.split_whitespace()) {
        let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/status")) else { continue };
        let get = |k: &str| s.lines().find(|l| l.starts_with(k)).map_or(0, |l| l.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() / 1024);
        rss += get("VmRSS:");
        hwm += get("VmHWM:");
    }
    (rss, hwm)
}

struct Ctx<'a> {
    root: &'a Path,
    vfs: &'a Vfs,
}

impl Ctx<'_> {
    fn file(&self, rel: &str) -> FileId {
        let p = AbsPathBuf::assert_utf8(self.root.join(rel));
        self.vfs.file_id(&VfsPath::from(p)).expect(rel).0
    }
    fn rel(&self, f: FileId) -> String {
        let p = self.vfs.file_path(f).to_string();
        p.strip_prefix(&format!("{}/", self.root.display())).unwrap_or(&p).to_string()
    }
    /// Byte offset of `needle` on 1-based `line1`. RA is natively UTF-8 offsets.
    fn pos(&self, rel: &str, line1: usize, needle: &str) -> FilePosition {
        let text = std::fs::read_to_string(self.root.join(rel)).unwrap();
        let line_start: usize = text.split_inclusive('\n').take(line1 - 1).map(str::len).sum();
        let col = text.lines().nth(line1 - 1).unwrap().find(needle).unwrap();
        FilePosition { file_id: self.file(rel), offset: TextSize::from((line_start + col) as u32) }
    }
}

macro_rules! timed {
    ($label:expr, $e:expr) => {{
        let t = Instant::now();
        let r = $e;
        println!("  [{:8.1} ms] {}", t.elapsed().as_secs_f64() * 1e3, $label);
        r
    }};
}

const FIX: RaFixtureConfig<'static> = RaFixtureConfig::default();

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = std::fs::canonicalize(&args[1])?;
    let all_features = args.iter().any(|a| a == "--all-features");
    let apply_rename = args.iter().any(|a| a == "--apply-rename");

    // --- load, as in analysis-stats / scip ---
    let t0 = Instant::now();
    let cargo_config = CargoConfig {
        sysroot: Some(RustLibSource::Discover),
        all_targets: true,
        set_test: true,
        features: if all_features {
            CargoFeatures::All
        } else {
            CargoFeatures::Selected { features: vec![], no_default_features: false }
        },
        ..Default::default()
    };
    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: true, // run build scripts (like scip)
        with_proc_macro_server: ProcMacroServerChoice::Sysroot,
        prefill_caches: true, // index everything up front, like scip / LSP cache priming
        num_worker_threads: std::thread::available_parallelism()?.get(),
        proc_macro_processes: 1,
    };
    let (db, vfs, pm) = load_workspace_at(&root, &cargo_config, &load_config, &|_| {})?;
    println!(
        "cold load (metadata + build scripts + sysroot + prefill caches): {:.1}s, proc-macro srv: {}",
        t0.elapsed().as_secs_f64(),
        if pm.is_some() { "spawned" } else { "NONE" }
    );
    let mut host = AnalysisHost::with_database(db);
    println!("RSS after load: {} MB (HWM {} MB)", rss_mb().0, rss_mb().1);
    let ctx = Ctx { root: &root, vfs: &vfs };

    let refs_cfg = FindAllRefsConfig { search_scope: None, ra_fixture: FIX, exclude_imports: false, exclude_tests: false };
    let ch_cfg = CallHierarchyConfig { exclude_tests: false, ra_fixture: FIX };
    let hover_cfg = HoverConfig {
        links_in_hover: false,
        memory_layout: None,
        documentation: false,
        keywords: false,
        format: HoverDocFormat::PlainText,
        max_trait_assoc_items_count: None,
        max_fields_count: None,
        max_enum_variants_count: None,
        max_subst_ty_len: SubstTyLen::Unlimited,
        show_drop_glue: false,
        ra_fixture: FIX,
    };
    let rename_cfg = RenameConfig { prefer_no_std: false, prefer_prelude: true, prefer_absolute: false, show_conflicts: false };

    let matcher = ctx.pos("crates/matcher/src/lib.rs", 546, "Matcher");
    let search_path = ctx.pos("crates/searcher/src/searcher/mod.rs", 643, "search_path");
    let alias_use = ctx.pos("crates/core/flags/defs.rs", 1256, "Separator");
    let find_at = ctx.pos("crates/matcher/src/lib.rs", 571, "find_at");
    let sink = ctx.pos("crates/core/search.rs", 388, "sink");

    let refs = |a: &ra_ap_ide::Analysis, p: FilePosition, label: &str| {
        let r = timed!(label, a.find_all_refs(p, &refs_cfg).unwrap().unwrap());
        let n: usize = r.iter().flat_map(|r| r.references.values()).map(Vec::len).sum();
        let files: std::collections::HashSet<_> = r.iter().flat_map(|r| r.references.keys()).collect();
        println!("      -> {n} refs in {} files", files.len());
    };

    println!("queries (first call, then warm repeat):");
    let a = host.analysis();
    refs(&a, matcher, "references Matcher trait");
    refs(&a, matcher, "references Matcher trait (warm)");
    refs(&a, search_path, "references Searcher::search_path");

    let d = timed!(
        "goto-definition through `use ContextSeparator as Separator`",
        a.goto_definition(alias_use, &GotoDefinitionConfig { ra_fixture: FIX }).unwrap().unwrap()
    );
    for nav in &d.info {
        let li = a.file_line_index(nav.file_id).unwrap();
        println!("      -> {}:{} ({})", ctx.rel(nav.file_id), li.line_col(nav.focus_or_full_range().start()).line + 1, nav.name);
    }

    let h = timed!(
        "hover (type) of `sink` local in core/search.rs",
        a.hover(&hover_cfg, FileRange { file_id: sink.file_id, range: TextRange::empty(sink.offset) }).unwrap().unwrap()
    );
    println!("      -> {}", h.info.markup.as_str().split_whitespace().collect::<Vec<_>>().join(" "));
    let expr = ctx.pos("crates/core/search.rs", 388, "p.sink_with_path");
    let len = "p.sink_with_path(&matcher, path)".len() as u32;
    let h = timed!(
        "hover range: type of expression `p.sink_with_path(&matcher, path)`",
        a.hover(&hover_cfg, FileRange { file_id: expr.file_id, range: TextRange::at(expr.offset, len.into()) }).unwrap()
    );
    println!("      -> {:?}", h.map(|h| h.info.markup.as_str().split_whitespace().collect::<Vec<_>>().join(" ")));

    let inc = timed!("incoming calls Searcher::search_path", a.incoming_calls(&ch_cfg, search_path).unwrap().unwrap());
    println!("      -> {:?}", inc.iter().map(|c| (c.target.name.to_string(), c.ranges.len())).collect::<Vec<_>>());

    timed!("prepare_rename Matcher::find_at", a.prepare_rename(find_at).unwrap().unwrap());
    let sc = timed!("rename Matcher::find_at -> find_at_renamed", a.rename(find_at, "find_at_renamed", &rename_cfg).unwrap().unwrap());
    let mut files: Vec<_> = sc.source_file_edits.iter().map(|(f, (e, _))| (ctx.rel(*f), e.len())).collect();
    files.sort();
    let mut crates: Vec<_> = files.iter().filter_map(|(f, _)| f.strip_prefix("crates/")?.split('/').next()).collect();
    crates.dedup();
    println!(
        "      -> {} edits in {} files, crates={crates:?}",
        files.iter().map(|f| f.1).sum::<usize>(),
        files.len()
    );
    if apply_rename {
        for (f, (edit, _)) in &sc.source_file_edits {
            let path = root.join(ctx.rel(*f));
            let mut text = std::fs::read_to_string(&path)?;
            edit.apply(&mut text);
            std::fs::write(&path, text)?;
        }
        println!("      applied rename edits to disk under {}", root.display());
    }
    drop(a);

    // --- incremental: in-memory edit, no reload ---
    let rel = "crates/core/search.rs";
    let f = ctx.file(rel);
    let mut text = std::fs::read_to_string(root.join(rel))?;
    text.push_str(
        "\nfn spike_new_caller(s: &mut grep::searcher::Searcher, m: &grep::regex::RegexMatcher) {\n    let mut sink = grep::searcher::sinks::UTF8(|_, _| Ok(true));\n    let _ = s.search_path(m, std::path::Path::new(\"x\"), &mut sink);\n}\n",
    );
    let mut change = ChangeWithProcMacros::default();
    change.change_file(f, Some(text));
    let dt = timed!("apply_change (append new caller to core/search.rs)", host.apply_change(change));
    let _ = dt;
    let a = host.analysis();
    let inc = timed!("incoming calls after edit", a.incoming_calls(&ch_cfg, search_path).unwrap().unwrap());
    println!("      -> {:?}", inc.iter().map(|c| (c.target.name.to_string(), c.ranges.len())).collect::<Vec<_>>());
    refs(&a, matcher, "references Matcher trait after edit");
    let lc = a.file_line_index(f).unwrap().line_col(sink.offset);
    let _ = LineCol { line: lc.line, col: lc.col };
    println!("RSS at end: {} MB (HWM {} MB)", rss_mb().0, rss_mb().1);
    Ok(())
}

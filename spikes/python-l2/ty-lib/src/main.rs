//! Spike: use ty's crates (ruff repo, pinned git rev) as a library from our own Rust crate.
//! Loads a Python project, then queries find-references / rename / hover / call hierarchy
//! for `httpx.Client.send`, applies an on-disk edit, and re-queries.
//!
//! Usage: ty-lib-spike <project_root>
use std::time::Instant;

use anyhow::{Context, Result};
use ruff_db::files::{File, system_path_to_file};
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{OsSystem, SystemPath, SystemPathBuf};
use ruff_text_size::{TextRange, TextSize};
use ty_ide::MarkupKind;
use ty_project::watch::{ChangeEvent, ChangedKind, CreatedKind};
use ty_project::{ProjectDatabase, ProjectMetadata};
use ty_python_semantic::Db as _;

fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    for key in ["VmHWM:"] {
        if let Some(l) = s.lines().find(|l| l.starts_with(key)) {
            let kb: f64 = l.split_whitespace().nth(1).unwrap().parse().unwrap();
            return kb / 1024.0;
        }
    }
    0.0
}

fn fmt_loc(db: &ProjectDatabase, root: &SystemPath, file: File, range: TextRange) -> String {
    let idx = line_index(db, file);
    let line = idx.line_index(range.start());
    let path = file.path(db).to_string();
    let rel = path.strip_prefix(&format!("{root}/")).unwrap_or(&path).to_string();
    format!("{rel}:{line}")
}

fn offset_of(db: &ProjectDatabase, file: File, needle: &str, token: &str) -> TextSize {
    let src = source_text(db, file);
    let at = src.as_str().find(needle).expect("needle");
    let tok = src.as_str()[at..].find(token).unwrap() + at;
    TextSize::try_from(tok).unwrap()
}

fn main() -> Result<()> {
    let root = SystemPathBuf::from(std::env::args().nth(1).context("project root")?);
    let t0 = Instant::now();
    let system = OsSystem::new(&root);
    let mut meta = ProjectMetadata::discover(&root, &system)?;
    meta.apply_configuration_files(&system)?;
    let mut db = ProjectDatabase::fallible(meta, system)?;
    println!("db_construct_ms={}", t0.elapsed().as_millis());

    // Full check = what an index-on-startup would cost (parses + infers every project file).
    let t = Instant::now();
    let diags = db.check();
    println!("full_check_ms={} diagnostics={} rss_hwm_mb={:.0}", t.elapsed().as_millis(), diags.len(), rss_mb());

    let client = system_path_to_file(&db, root.join("httpx/_client.py"))?;
    let fixture = system_path_to_file(&db, root.join("fixture_semcode.py"))?;
    let off = offset_of(&db, client, "    def send(", "send");
    let pf = db.program_file(client);

    let refs = |db: &ProjectDatabase, label: &str| -> Vec<String> {
        let t = Instant::now();
        let r = ty_ide::find_references(db, db.program_file(client), off, false).unwrap_or_default();
        let mut v: Vec<String> = r.iter().map(|r| fmt_loc(db, &root, r.file(), r.range())).collect();
        v.sort();
        v.dedup();
        println!("{label}_ms={} n={}", t.elapsed().as_millis(), v.len());
        v
    };
    let v = refs(&db, "refs_cold");
    for l in &v {
        println!("  ref {l}");
    }
    refs(&db, "refs_warm");

    // From the alias use site in the fixture.
    let alias_off = offset_of(&db, fixture, "return c.send(req)", "send");
    let r = ty_ide::find_references(&db, db.program_file(fixture), alias_off, true).unwrap_or_default();
    println!("refs_from_alias_site n={}", r.len());

    // Rename from base and from override.
    for (label, file, o) in [
        ("base", client, off),
        ("override", fixture, offset_of(&db, fixture, "    def send(", "send")),
    ] {
        let t = Instant::now();
        let edits = ty_ide::rename(&db, db.program_file(file), o, "send_req").unwrap_or_default();
        let mut locs: Vec<String> = edits.iter().map(|r| fmt_loc(&db, &root, r.file(), r.range())).collect();
        locs.sort();
        println!("rename_{label}_ms={} n_edits={} {:?}", t.elapsed().as_millis(), locs.len(), locs);
    }

    // Hover on `client` in `with httpx.Client() as client:`.
    for (needle, tok) in [("with httpx.Client() as client", "client"), ("r = client.send", "r"), ("c = h.Client()", "c")] {
        let o = offset_of(&db, fixture, needle, tok);
        if let Some(h) = ty_ide::hover(&db, db.program_file(fixture), o) {
            let s = h.display(&db, MarkupKind::PlainText).to_string();
            println!("hover[{needle}] = {}", s.lines().take(3).collect::<Vec<_>>().join(" | "));
        }
    }

    // Call hierarchy incoming.
    let t = Instant::now();
    let calls = ty_ide::incoming_calls(&db, pf, off);
    let mut callers: Vec<String> = calls
        .iter()
        .map(|c| format!("{}:{}", fmt_loc(&db, &root, c.from.file, c.from.selection_range), c.from.name))
        .collect();
    callers.sort();
    println!("incoming_calls_ms={} n={} {:?}", t.elapsed().as_millis(), callers.len(), callers);

    // Incremental: write a new file + modify an existing one on disk, notify db, re-query.
    let newp = root.join("fixture_ondisk.py");
    std::fs::write(newp.as_std_path(), "import httpx\n\ndef ondisk(req: httpx.Request):\n    return httpx.Client().send(req)\n")?;
    let fx_path = root.join("fixture_semcode.py");
    let mut src = std::fs::read_to_string(fx_path.as_std_path())?;
    let orig = src.clone();
    src.push_str("\n\ndef added(req: httpx.Request):\n    return httpx.Client().send(req)\n");
    std::fs::write(fx_path.as_std_path(), &src)?;
    let t = Instant::now();
    db.apply_changes(&[
        ChangeEvent::Created { path: newp.clone(), kind: CreatedKind::File },
        ChangeEvent::Changed { path: fx_path.clone(), kind: ChangedKind::FileContent },
    ]);
    println!("apply_changes_ms={}", t.elapsed().as_millis());
    let v2 = refs(&db, "refs_after_edit");
    println!(
        "  sees_new_file={} sees_edit={}",
        v2.iter().any(|l| l.starts_with("fixture_ondisk.py:4")),
        v2.iter().any(|l| l.starts_with("fixture_semcode.py:40"))
    );
    std::fs::remove_file(newp.as_std_path())?;
    std::fs::write(fx_path.as_std_path(), orig)?;
    println!("rss_hwm_mb={:.0} total_ms={}", rss_mb(), t0.elapsed().as_millis());
    Ok(())
}

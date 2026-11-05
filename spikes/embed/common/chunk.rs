// Shared spike helpers: crude Python function chunker, queries, top-k, RSS.
// Included via `#[path = "../../common/chunk.rs"] mod chunk;` from each runtime crate.
#![allow(dead_code)]

use regex::Regex;
use std::path::Path;

pub struct Chunk {
    pub title: String, // relpath::Class.func
    pub code: String,
}

pub const QUERY_PREFIX: &str = "task: code retrieval | query: ";

pub const QUERIES: &[&str] = &[
    "where are redirects followed",
    "parse proxy URL from environment",
    "retry on connection failure",
    "decode gzip/brotli content",
    "build multipart form body",
    "basic and digest authentication challenge",
    "percent-encode URL path and query",
];

pub fn doc_text(c: &Chunk) -> String {
    format!("title: {} | text: {}", c.title, c.code)
}

/// Splits every `def` (incl. methods/nested) into a chunk spanning to the next line whose
/// indent is <= the def's indent (after the signature ends). Crude, but fine for a spike.
pub fn chunk_python_dir(root: &Path) -> Vec<Chunk> {
    let mut files: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "py"))
        .map(|e| e.into_path())
        .collect();
    files.sort();
    let re_def = Regex::new(r"^(\s*)(?:async\s+)?def\s+(\w+)").unwrap();
    let re_class = Regex::new(r"^(\s*)class\s+(\w+)").unwrap();
    let mut out = Vec::new();
    for f in files {
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        let rel = f.strip_prefix(root).unwrap_or(&f).display().to_string();
        let lines: Vec<&str> = src.lines().collect();
        let indent = |l: &str| l.len() - l.trim_start().len();
        let mut classes: Vec<(usize, String)> = Vec::new();
        let mut defs: Vec<(usize, String)> = Vec::new(); // enclosing defs for nested qualnames
        for i in 0..lines.len() {
            let l = lines[i];
            if l.trim().is_empty() || l.trim_start().starts_with('#') {
                continue;
            }
            let ind = indent(l);
            classes.retain(|(ci, _)| *ci < ind);
            defs.retain(|(di, _)| *di < ind);
            if let Some(c) = re_class.captures(l) {
                classes.push((c[1].len(), c[2].to_string()));
                continue;
            }
            let Some(c) = re_def.captures(l) else { continue };
            let name = c[2].to_string();
            // signature end: first line (from i) whose code part ends with ':'
            let mut j = i;
            while j < lines.len() {
                let code = lines[j].split('#').next().unwrap_or("").trim_end();
                if code.ends_with(':') {
                    break;
                }
                j += 1;
            }
            let mut end = lines.len();
            for (k, lk) in lines.iter().enumerate().skip(j + 1) {
                if !lk.trim().is_empty() && indent(lk) <= ind {
                    end = k;
                    break;
                }
            }
            let mut qual: Vec<String> = classes.iter().map(|(_, n)| n.clone()).collect();
            qual.extend(defs.iter().map(|(_, n)| n.clone()));
            qual.push(name.clone());
            let code = lines[i..end].join("\n").trim_end().to_string();
            out.push(Chunk { title: format!("{}::{}", rel, qual.join(".")), code });
            defs.push((ind, name));
        }
    }
    out
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
}

/// Brute-force cosine top-k over row-major normalized matrix, optionally truncated to `dim` (MRL).
pub fn topk(mat: &[f32], full_dim: usize, q: &[f32], dim: usize, k: usize) -> Vec<(usize, f32)> {
    let mut qd = q[..dim].to_vec();
    normalize(&mut qd);
    let mut scores: Vec<(usize, f32)> = mat
        .chunks_exact(full_dim)
        .enumerate()
        .map(|(i, row)| {
            let r = &row[..dim];
            let n = r.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            (i, r.iter().zip(&qd).map(|(a, b)| a * b).sum::<f32>() / n)
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    scores.truncate(k);
    scores
}

pub fn rss_mb() -> (f64, f64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 1024.0
    };
    (get("VmRSS:"), get("VmHWM:"))
}

pub fn write_f32(path: &Path, v: &[f32]) {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    std::fs::write(path, bytes).unwrap();
}

pub fn read_f32(path: &Path) -> Option<Vec<f32>> {
    let b = std::fs::read(path).ok()?;
    Some(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
}

pub fn print_results(chunks: &[Chunk], mat: &[f32], dim_full: usize, qembs: &[Vec<f32>], dims: &[usize]) {
    for (qi, q) in QUERIES.iter().enumerate() {
        println!("\nQ: {q}");
        for &d in dims {
            let r = topk(mat, dim_full, &qembs[qi], d, 3);
            let s: Vec<String> = r.iter().map(|(i, sc)| format!("{} ({:.3})", chunks[*i].title, sc)).collect();
            println!("  [{d}d] {}", s.join("  |  "));
        }
    }
}

// EmbeddingGemma 2 via ONNX Runtime (ort 2.0.0-rc.14, statically linked ORT 1.31) + HF tokenizers.
//
// usage:
//   ort-embed info   <model.onnx>
//   ort-embed bench  <model.onnx> <tokenizer.json> [threads]
//   ort-embed search <model.onnx> <tokenizer.json> <corpus_dir> <out_prefix> [max_tokens]
#[path = "../../common/chunk.rs"]
mod chunk;

use anyhow::{Result, anyhow};
use chunk::*;
use ndarray::Array2;
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::TensorRef;
use std::path::Path;
use std::time::Instant;
use tokenizers::Tokenizer;

const EOS: u32 = 1;
const PAD: u32 = 0;

fn load(model: &str, threads: usize) -> Result<Session> {
    let t = Instant::now();
    let env = ort::init().build()?;
    let mut b = Session::builder(&env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow!("{e}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow!("{e}"))?;
    if let Ok(p) = std::env::var("ORT_PROFILE") {
        b = b.with_profiling(p).map_err(|e| anyhow!("{e}"))?;
    }
    let s = b.commit_from_file(model)?;
    let (rss, _) = rss_mb();
    eprintln!("load {model}: {:.2}s, RSS {:.0} MB", t.elapsed().as_secs_f64(), rss);
    Ok(s)
}

fn encode(tok: &Tokenizer, text: &str, max: usize) -> Vec<u32> {
    let mut ids = tok.encode(text, true).unwrap().get_ids().to_vec();
    if ids.len() > max {
        ids.truncate(max - 1);
        ids.push(EOS);
    }
    ids
}

/// Embeds one padded batch; returns row-major [B, 768] (already L2-normalized by the graph).
fn embed_batch(s: &mut Session, batch: &[Vec<u32>]) -> Result<Vec<f32>> {
    let maxlen = batch.iter().map(|v| v.len()).max().unwrap();
    let mut ids = Array2::<i64>::from_elem((batch.len(), maxlen), PAD as i64);
    let mut mask = Array2::<i64>::zeros((batch.len(), maxlen));
    for (i, v) in batch.iter().enumerate() {
        for (j, &t) in v.iter().enumerate() {
            ids[[i, j]] = t as i64;
            mask[[i, j]] = 1;
        }
    }
    // text-only: the multimodal graph still requires the (empty) media feature inputs
    let empty = Array2::<f32>::zeros((0, 512));
    let out = s.run(ort::inputs![
        "input_ids" => TensorRef::from_array_view(&ids)?,
        "attention_mask" => TensorRef::from_array_view(&mask)?,
        "image_features" => TensorRef::from_array_view(&empty)?,
        "video_features" => TensorRef::from_array_view(&empty)?,
        "audio_features" => TensorRef::from_array_view(&empty)?,
    ])?;
    let (shape, data) = out["sentence_embedding"].try_extract_tensor::<f32>()?;
    assert_eq!(shape[1], 768);
    let mut v = data.to_vec();
    for row in v.chunks_mut(768) {
        let n: f32 = row.iter().map(|x| x * x).sum::<f32>().sqrt();
        if (n - 1.0).abs() > 1e-3 && std::env::var("WARN_NORM").is_ok() { eprintln!("norm {n}"); }
        normalize(row);
    }
    Ok(v)
}

/// Embeds all sequences, sorting by length to minimize padding; returns in original order.
fn embed_all(s: &mut Session, seqs: &[Vec<u32>], bs: usize) -> Result<Vec<f32>> {
    let mut order: Vec<usize> = (0..seqs.len()).collect();
    order.sort_by_key(|&i| seqs[i].len());
    let mut out = vec![0f32; seqs.len() * 768];
    for grp in order.chunks(bs) {
        let batch: Vec<Vec<u32>> = grp.iter().map(|&i| seqs[i].clone()).collect();
        let e = embed_batch(s, &batch)?;
        for (k, &i) in grp.iter().enumerate() {
            out[i * 768..(i + 1) * 768].copy_from_slice(&e[k * 768..(k + 1) * 768]);
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let corpus = "/home/pavitra/Playground/semcode/corpus/httpx";
    match a.get(1).map(String::as_str) {
        Some("info") => {
            let s = load(&a[2], 8)?;
            for i in s.inputs() {
                println!("in  {} {:?}", i.name(), i.dtype());
            }
            for o in s.outputs() {
                println!("out {} {:?}", o.name(), o.dtype());
            }
        }
        Some("profile") => {
            // one 200-token chunk x 5 runs, then dump the ORT profile
            let tok = Tokenizer::from_file(&a[3]).map_err(|e| anyhow!("{e}"))?;
            let mut s = load(&a[2], 8)?;
            let chunks = chunk_python_dir(Path::new(corpus));
            let seq = chunks.iter().map(|c| encode(&tok, &doc_text(c), 200)).find(|v| v.len() == 200).unwrap();
            for _ in 0..5 { embed_batch(&mut s, &[seq.clone()])?; }
            println!("{}", s.end_profiling()?);
        }
        Some("bench") => {
            let threads: usize = a.get(4).map(|x| x.parse().unwrap()).unwrap_or(8);
            let tok = Tokenizer::from_file(&a[3]).map_err(|e| anyhow!("{e}"))?;
            let mut s = load(&a[2], threads)?;
            // ~200-token real code chunks: functions with >=200 tokens, truncated to exactly 200.
            let chunks = chunk_python_dir(Path::new(corpus));
            let seqs: Vec<Vec<u32>> = chunks
                .iter()
                .map(|c| encode(&tok, &doc_text(c), 200))
                .filter(|v| v.len() == 200)
                .take(256)
                .collect();
            println!("bench set: {} chunks x 200 tokens, threads={threads}", seqs.len());
            // single-query latency
            let q = encode(&tok, &format!("{QUERY_PREFIX}parse proxy URL from environment"), 64);
            embed_batch(&mut s, &[q.clone()])?; // warmup
            let mut lat = vec![];
            for _ in 0..20 {
                let t = Instant::now();
                embed_batch(&mut s, &[q.clone()])?;
                lat.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            lat.sort_by(|a, b| a.total_cmp(b));
            println!("query ({} tok) latency: p50 {:.1} ms, p90 {:.1} ms", q.len(), lat[10], lat[18]);
            for bs in [1usize, 4, 8, 16, 32, 64] {
                let n = (if bs == 1 { 32 } else { 128 }).min(seqs.len());
                embed_all(&mut s, &seqs[..8.min(seqs.len())], bs)?; // warmup
                let t = Instant::now();
                embed_all(&mut s, &seqs[..n], bs)?;
                let dt = t.elapsed().as_secs_f64();
                let (rss, hwm) = rss_mb();
                println!(
                    "batch {bs:>3}: {:>6.1} chunks/s ({:>6.0} tok/s)  RSS {:.0} MB  peak {:.0} MB",
                    n as f64 / dt,
                    n as f64 * 200.0 / dt,
                    rss,
                    hwm
                );
            }
        }
        Some("search") => {
            let tok = Tokenizer::from_file(&a[3]).map_err(|e| anyhow!("{e}"))?;
            let mut s = load(&a[2], 8)?;
            let out_prefix = &a[5];
            let max: usize = a.get(6).map(|x| x.parse().unwrap()).unwrap_or(512);
            let chunks = chunk_python_dir(Path::new(&a[4]));
            let seqs: Vec<Vec<u32>> = chunks.iter().map(|c| encode(&tok, &doc_text(c), max)).collect();
            let ntok: usize = seqs.iter().map(|v| v.len()).sum();
            let t = Instant::now();
            let mat = embed_all(&mut s, &seqs, 16)?;
            let dt = t.elapsed().as_secs_f64();
            let (rss, hwm) = rss_mb();
            println!(
                "indexed {} chunks ({} tokens, mean {:.0}, max_len {max}) in {:.1}s = {:.1} chunks/s; RSS {:.0} MB peak {:.0} MB",
                chunks.len(), ntok, ntok as f64 / chunks.len() as f64, dt, chunks.len() as f64 / dt, rss, hwm
            );
            let qseqs: Vec<Vec<u32>> = QUERIES.iter().map(|q| encode(&tok, &format!("{QUERY_PREFIX}{q}"), 128)).collect();
            let qm = embed_all(&mut s, &qseqs, 1)?;
            let qembs: Vec<Vec<f32>> = qm.chunks(768).map(|c| c.to_vec()).collect();
            write_f32(Path::new(&format!("{out_prefix}.docs.f32")), &mat);
            write_f32(Path::new(&format!("{out_prefix}.queries.f32")), &qm);
            print_results(&chunks, &mat, 768, &qembs, &[768, 256]);
        }
        _ => eprintln!("usage: see header comment"),
    }
    Ok(())
}

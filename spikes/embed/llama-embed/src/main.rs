// EmbeddingGemma 2 via llama.cpp (llama-cpp-2 0.1.159 safe bindings; llama-cpp-sys-2 patched to
// upstream llama.cpp 9cf967a because the published crate vendors a pre-`gemma-embedding2` commit).
//
// usage:
//   llama-embed bench  <model.gguf> [threads]
//   llama-embed search <model.gguf> <corpus_dir> <out_prefix> [max_tokens]
#[path = "../../common/chunk.rs"]
mod chunk;

use anyhow::{Result, anyhow};
use chunk::*;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::token::LlamaToken;
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Instant;

const MAX_BATCH_TOKENS: usize = 4096;
const MAX_SEQS: usize = 64;

fn encode(model: &LlamaModel, text: &str, max: usize) -> Vec<LlamaToken> {
    let vocab = model.vocab();
    let mut ids = vocab.tokenize(text.as_bytes(), true, false);
    // HF tokenizer appends <eos>; make llama.cpp tokenization match it.
    let eos = vocab.eos();
    if ids.last() != Some(&eos) {
        ids.push(eos);
    }
    if ids.len() > max {
        ids.truncate(max - 1);
        ids.push(eos);
    }
    ids
}

/// Embeds a group of sequences in one llama_decode call (one seq_id per sequence).
fn embed_group(ctx: &mut LlamaContext, seqs: &[&Vec<LlamaToken>]) -> Result<Vec<f32>> {
    let n: usize = seqs.iter().map(|s| s.len()).sum();
    let mut batch = LlamaBatch::new(n, seqs.len() as i32);
    for (i, s) in seqs.iter().enumerate() {
        batch.add_sequence(s, i as i32, true)?;
    }
    ctx.clear_kv_cache();
    ctx.decode(&mut batch).map_err(|e| anyhow!("decode: {e}"))?;
    let mut out = Vec::with_capacity(seqs.len() * 768);
    for i in 0..seqs.len() {
        let e = ctx.embeddings_seq_ith(i as i32).map_err(|_| anyhow!("no embedding"))?;
        assert_eq!(e.len(), 768, "expected 768-d output (512->768 projection)");
        let mut v = e.to_vec();
        normalize(&mut v);
        out.extend(v);
    }
    Ok(out)
}

/// Packs sequences (sorted by length) into groups of <= MAX_BATCH_TOKENS / bs sequences.
fn embed_all(ctx: &mut LlamaContext, seqs: &[Vec<LlamaToken>], bs: usize) -> Result<Vec<f32>> {
    let mut order: Vec<usize> = (0..seqs.len()).collect();
    order.sort_by_key(|&i| seqs[i].len());
    let mut out = vec![0f32; seqs.len() * 768];
    let mut i = 0;
    while i < order.len() {
        let mut grp = vec![];
        let mut ntok = 0;
        while i < order.len() && grp.len() < bs && ntok + seqs[order[i]].len() <= MAX_BATCH_TOKENS {
            ntok += seqs[order[i]].len();
            grp.push(order[i]);
            i += 1;
        }
        let refs: Vec<&Vec<LlamaToken>> = grp.iter().map(|&k| &seqs[k]).collect();
        let e = embed_group(ctx, &refs)?;
        for (k, &idx) in grp.iter().enumerate() {
            out[idx * 768..(idx + 1) * 768].copy_from_slice(&e[k * 768..(k + 1) * 768]);
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let corpus = "/home/pavitra/Playground/semcode/corpus/httpx";
    let mut backend = LlamaBackend::init()?;
    if std::env::var("LLAMA_LOG").is_err() {
        backend.void_logs();
    }
    let threads: i32 = match a[1].as_str() {
        "bench" => a.get(3).map(|x| x.parse().unwrap()).unwrap_or(8),
        _ => 8,
    };
    let t = Instant::now();
    let model = LlamaModel::load_from_file(&backend, &a[2], &LlamaModelParams::default())?;
    let cparams = LlamaContextParams::default()
        .with_embeddings(true)
        .with_pooling_type(LlamaPoolingType::Mean)
        .with_n_ctx(NonZeroU32::new(MAX_BATCH_TOKENS as u32))
        .with_n_batch(MAX_BATCH_TOKENS as u32)
        .with_n_ubatch(MAX_BATCH_TOKENS as u32) // non-causal: whole sequence must fit one ubatch
        .with_n_seq_max(MAX_SEQS as u32)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let mut ctx = model.new_context(&backend, cparams)?;
    let (rss, _) = rss_mb();
    eprintln!("load {}: {:.2}s, RSS {:.0} MB, n_embd_out {}", a[2], t.elapsed().as_secs_f64(), rss, model.n_embd_out());

    match a[1].as_str() {
        "bench" => {
            let chunks = chunk_python_dir(Path::new(corpus));
            let seqs: Vec<Vec<LlamaToken>> = chunks
                .iter()
                .map(|c| encode(&model, &doc_text(c), 200))
                .filter(|v| v.len() == 200)
                .take(256)
                .collect();
            println!("bench set: {} chunks x 200 tokens, threads={threads}", seqs.len());
            let q = encode(&model, &format!("{QUERY_PREFIX}parse proxy URL from environment"), 64);
            embed_all(&mut ctx, &[q.clone()], 1)?;
            let mut lat = vec![];
            for _ in 0..20 {
                let t = Instant::now();
                embed_all(&mut ctx, &[q.clone()], 1)?;
                lat.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            lat.sort_by(|a, b| a.total_cmp(b));
            println!("query ({} tok) latency: p50 {:.1} ms, p90 {:.1} ms", q.len(), lat[10], lat[18]);
            for bs in [1usize, 4, 8, 16] {
                let n = (if bs == 1 { 32 } else { 128 }).min(seqs.len());
                embed_all(&mut ctx, &seqs[..8], bs)?;
                let t = Instant::now();
                embed_all(&mut ctx, &seqs[..n], bs)?;
                let dt = t.elapsed().as_secs_f64();
                let (rss, hwm) = rss_mb();
                println!(
                    "batch {bs:>3}: {:>6.1} chunks/s ({:>6.0} tok/s)  RSS {:.0} MB  peak {:.0} MB",
                    n as f64 / dt, n as f64 * 200.0 / dt, rss, hwm
                );
            }
        }
        "search" => {
            let out_prefix = &a[4];
            let max: usize = a.get(5).map(|x| x.parse().unwrap()).unwrap_or(512);
            let chunks = chunk_python_dir(Path::new(&a[3]));
            let seqs: Vec<Vec<LlamaToken>> = chunks.iter().map(|c| encode(&model, &doc_text(c), max)).collect();
            let ntok: usize = seqs.iter().map(|v| v.len()).sum();
            let t = Instant::now();
            let mat = embed_all(&mut ctx, &seqs, 16)?;
            let dt = t.elapsed().as_secs_f64();
            let (rss, hwm) = rss_mb();
            println!(
                "indexed {} chunks ({} tokens) in {:.1}s = {:.1} chunks/s, {:.0} tok/s; RSS {:.0} MB peak {:.0} MB",
                chunks.len(), ntok, dt, chunks.len() as f64 / dt, ntok as f64 / dt, rss, hwm
            );
            let qseqs: Vec<Vec<LlamaToken>> =
                QUERIES.iter().map(|q| encode(&model, &format!("{QUERY_PREFIX}{q}"), 128)).collect();
            let qm = embed_all(&mut ctx, &qseqs, 1)?;
            let qembs: Vec<Vec<f32>> = qm.chunks(768).map(|c| c.to_vec()).collect();
            write_f32(Path::new(&format!("{out_prefix}.docs.f32")), &mat);
            write_f32(Path::new(&format!("{out_prefix}.queries.f32")), &qm);
            print_results(&chunks, &mat, 768, &qembs, &[768, 256]);
        }
        _ => eprintln!("usage: see header"),
    }
    Ok(())
}

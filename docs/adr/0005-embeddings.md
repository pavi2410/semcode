# ADR-0005: Natural-language search with EmbeddingGemma 2

Status: proposed. The spike was stopped early; the data below is partial. Evidence is in `spikes/embed/out/`.

## What we know
- `google/embeddinggemma-2` (2026-10-06, Apache 2.0): text tower 270M, 768-dim, Matryoshka
  (256-dim tested), 8k context. Official ONNX (`model.onnx`, `model_q4.onnx`, …) and GGUF
  (`BF16`, `Q8_0`) builds exist.
- **ort (ONNX Runtime) from Rust works.** fp32 loads in 0.54 s at 641 MB RSS. q4 loads in 0.28 s at 265 MB RSS.
  llama.cpp with GGUF Q8_0 loads in 0.61 s at 452 MB. Its throughput was not measured.
- **Quality on httpx** (1134 function chunks): good on concrete intents ("decode gzip/brotli" →
  `BrotliDecoder.decode`; "parse proxy URL from environment" → `get_environment_proxies`).
  Results for vague intents ("retry on connection failure") are plausible but not precise. q4 ≈ fp32, and 256-dim ≈ 768-dim.
  Tests often outrank implementation, so we'll need structure-aware re-ranking.
- **Throughput is the constraint:** about 5.4 chunks/s with peak RSS around 2.6 GB on a heavily loaded machine
  (other spikes were compiling). This needs to be re-measured on an idle machine before deciding anything.

## Proposed direction
- Runtime: `ort` with the q4 ONNX model behind an `Embedder` trait. Re-measure llama.cpp before
  committing.
- Index at 256 dims (Matryoshka) with brute-force SIMD cosine, which is enough for repos with
  fewer than about 1M chunks. Store vectors in the daemon's index keyed by chunk content hash,
  so unchanged code is never re-embedded.
- Chunk by symbol (functions and classes from L1 outlines), not fixed windows.
- Index in the background at low priority. `ask` reports coverage (`indexed 61%`) and does not block.
- Re-rank with structural signals (test vs. source, call-graph centrality, symbol kind).

## Open questions before acceptance
1. Throughput on an idle machine with tuned batch size and thread count, and q4 vs. Q8 GGUF.
2. Whether a bounded batch size brings peak memory under about 500 MB.
3. The model download story: fetch it on first `ask`, or install it explicitly with `semcode model pull`.

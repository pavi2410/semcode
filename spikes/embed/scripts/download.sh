#!/usr/bin/env bash
# Downloads EmbeddingGemma 2 artifacts (ungated, Apache-2.0) into ../models/
set -euo pipefail
M="$(cd "$(dirname "$0")/.." && pwd)/models"
mkdir -p "$M/onnx2/onnx" "$M/gguf2"
O=https://huggingface.co/onnx-community/embeddinggemma-2-ONNX/resolve/main
G=https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/main
get() { curl -sSL --retry 3 -C - -o "$2" "$1" || curl -sSL --retry 3 -o "$2" "$1"; }
for f in tokenizer.json tokenizer_config.json config.json onnx/model.onnx onnx/model_quantized.onnx onnx/model_q4.onnx; do get "$O/$f" "$M/onnx2/$f"; done
get "$O/onnx/model.onnx_data" "$M/onnx2/onnx/model.onnx_data" &
get "$O/onnx/model_quantized.onnx_data" "$M/onnx2/onnx/model_quantized.onnx_data" &
get "$O/onnx/model_q4.onnx_data" "$M/onnx2/onnx/model_q4.onnx_data" &
get "$G/embeddinggemma-2-Q8_0.gguf" "$M/gguf2/embeddinggemma-2-Q8_0.gguf" &
get "$G/embeddinggemma-2-BF16.gguf" "$M/gguf2/embeddinggemma-2-BF16.gguf" &
wait
echo ALL_DONE

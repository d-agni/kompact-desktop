#!/usr/bin/env bash
# 19f: the small language model Siva asks about requests his rules don't know: Qwen2.5 1.5B
# Instruct (Q4_K_M, 1.1 GB, Apache-2.0) from Qwen's own page on Hugging Face, checked against the
# SHA-256 Hugging Face publishes. Siva himself never goes online; this script is the one time you
# fetch it. It runs on the CPU in Ubuntu's llama.cpp (`sudo apt install llama.cpp-tools`).
#
# 21c: `work` fetches the model for tasks when there's no cloud AI: Qwen2.5-Coder 3B Instruct
# (Q4_K_M, 2.1 GB), under the Qwen RESEARCH LICENSE: non-commercial use only (research or
# evaluation); for commercial use, ask Alibaba Cloud for a license. Fetching it means you accept
# that license (https://huggingface.co/Qwen/Qwen2.5-Coder-3B-Instruct-GGUF/blob/main/LICENSE).
#
#   shell/tools/get-assistant-model.sh          # the 1.5B, for understanding requests
#   shell/tools/get-assistant-model.sh work     # the Coder 3B, for tasks
#
# Into ~/.local/share/kompact/llm/.
set -euo pipefail

case "${1:-}" in
  "")
    name=qwen2.5-1.5b-instruct-q4_k_m.gguf
    sha=6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e
    url="https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/$name"
    size="1.1 GB"
    ;;
  work)
    name=qwen2.5-coder-3b-instruct-q4_k_m.gguf
    sha=724fb256bec1ff062b2f65e4569e871ad2e95ab2a3989723d1769c54294730b7
    url="https://huggingface.co/Qwen/Qwen2.5-Coder-3B-Instruct-GGUF/resolve/main/$name"
    size="2.1 GB"
    echo "Qwen2.5-Coder 3B is under the Qwen RESEARCH LICENSE: non-commercial use only."
    echo "Qwen is licensed under the Qwen RESEARCH LICENSE AGREEMENT, Copyright (c) Alibaba Cloud. All Rights Reserved."
    ;;
  *)
    echo "usage: $0 [work]" >&2
    exit 2
    ;;
esac
dir="${XDG_DATA_HOME:-$HOME/.local/share}/kompact/llm"
file="$dir/$name"

if [[ -f "$file" ]] && echo "$sha  $file" | sha256sum -c --quiet - 2>/dev/null; then
  echo "Already here: $file"
else
  mkdir -p "$dir"
  echo "Fetching $url ($size)"
  curl -fL --progress-bar -o "$file.part" "$url"
  if ! echo "$sha  $file.part" | sha256sum -c --quiet -; then
    rm -f "$file.part"
    echo "The download doesn't match its checksum; nothing kept." >&2
    exit 1
  fi
  mv "$file.part" "$file"
  echo "Saved $file"
fi

if [[ "${1:-}" == work ]]; then
  command -v llama-server >/dev/null || echo "Now: sudo apt install llama.cpp-tools"
else
  command -v llama-completion >/dev/null || echo "Now: sudo apt install llama.cpp-tools"
fi

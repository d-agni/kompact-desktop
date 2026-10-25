#!/usr/bin/env bash
# 19f: the small language model Siva asks about requests his rules don't know: Qwen2.5 1.5B
# Instruct (Q4_K_M, 1.1 GB, Apache-2.0) from Qwen's own page on Hugging Face, checked against the
# SHA-256 Hugging Face publishes. Siva himself never goes online; this script is the one time you
# fetch it. It runs on the CPU in Ubuntu's llama.cpp (`sudo apt install llama.cpp-tools`).
#
#   shell/tools/get-assistant-model.sh
#
# Into ~/.local/share/kompact/llm/.
set -euo pipefail

name=qwen2.5-1.5b-instruct-q4_k_m.gguf
sha=6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e
url="https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/$name"
dir="${XDG_DATA_HOME:-$HOME/.local/share}/kompact/llm"
file="$dir/$name"

if [[ -f "$file" ]] && echo "$sha  $file" | sha256sum -c --quiet - 2>/dev/null; then
  echo "Already here: $file"
else
  mkdir -p "$dir"
  echo "Fetching $url (1.1 GB)"
  curl -fL --progress-bar -o "$file.part" "$url"
  if ! echo "$sha  $file.part" | sha256sum -c --quiet -; then
    rm -f "$file.part"
    echo "The download doesn't match its checksum; nothing kept." >&2
    exit 1
  fi
  mv "$file.part" "$file"
  echo "Saved $file"
fi

command -v llama-completion >/dev/null || echo "Now: sudo apt install llama.cpp-tools"

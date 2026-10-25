#!/usr/bin/env bash
# 19h-2: the voice Siva speaks with: Piper (a local text-to-speech program, 2023.11.14-2) and its
# British English "Alan" voice. Siva himself never goes online; this script is the one time you
# fetch them, and each file is checked against a pinned SHA-256 before it's used (the voice's
# hashes are the ones Hugging Face publishes; GitHub publishes none for that Piper build, so its
# hash was taken on 2026-10-24 and pinned, together with its size).
#
#   shell/tools/get-speaking-voice.sh
#
# Into ~/.local/share/kompact/piper/ (the program) and …/piper/voices/ (the voice), ~90 MB.
set -euo pipefail

dir="${XDG_DATA_HOME:-$HOME/.local/share}/kompact/piper"
piper_url="https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x86_64.tar.gz"
piper_sha=a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992
voice=en_GB-alan-medium
voice_url="https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_GB/alan/medium"
onnx_sha=0a309668932205e762801f1efc2736cd4b0120329622adf62be09e56339d3330
json_sha=c0f0d124e5895c00e7c03b35dcc8287f319a6998a365b182deb5c8e752ee8c1e

# fetch URL FILE SHA256: download unless already there and right; keep it only if it matches.
fetch() {
  local url="$1" file="$2" sha="$3"
  if [[ -f "$file" ]] && echo "$sha  $file" | sha256sum -c --quiet - 2>/dev/null; then
    echo "Already here: $file"
    return
  fi
  echo "Fetching $url"
  curl -fL --progress-bar -o "$file.part" "$url"
  if ! echo "$sha  $file.part" | sha256sum -c --quiet -; then
    rm -f "$file.part"
    echo "$file doesn't match its checksum; nothing kept." >&2
    exit 1
  fi
  mv "$file.part" "$file"
}

mkdir -p "$dir/voices"
fetch "$piper_url" "$dir/piper_linux_x86_64.tar.gz" "$piper_sha"
if [[ ! -x "$dir/piper/piper" ]]; then
  tar -xzf "$dir/piper_linux_x86_64.tar.gz" -C "$dir"
fi
fetch "$voice_url/$voice.onnx" "$dir/voices/$voice.onnx" "$onnx_sha"
fetch "$voice_url/$voice.onnx.json" "$dir/voices/$voice.onnx.json" "$json_sha"

bytes=$(echo "Hello. I'm ready." | "$dir/piper/piper" --model "$dir/voices/$voice.onnx" --output_raw 2>/dev/null | wc -c)
if (( bytes > 10000 )); then
  echo "Ready: Siva speaks with the $voice voice."
else
  echo "Piper didn't produce sound; see: $dir/piper/piper --help" >&2
  exit 1
fi

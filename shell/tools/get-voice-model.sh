#!/usr/bin/env bash
# 19d: the speech model Siva's voice uses (whisper.cpp, English). Siva himself never goes online;
# this script is the one time you fetch the model, from whisper.cpp's own page on Hugging Face,
# and it is checked against whisper.cpp's published SHA-1 before it's used.
#
#   shell/tools/get-voice-model.sh            # base.en (142 MB): fast, good for commands
#   shell/tools/get-voice-model.sh small.en   # small.en (466 MB): more accurate, ~3x slower
#
# Into ~/.local/share/kompact/whisper/ggml-<model>.bin. Also needs `sudo apt install whisper.cpp`.
set -euo pipefail

model="${1:-base.en}"
case "$model" in
  tiny.en) sha=c78c86eb1a8faa21b369bcd33207cc90d64ae9df ;;
  base.en) sha=137c40403d78fd54d454da0f9bd998f78703390c ;;
  small.en) sha=db8a495a91d927739e50b3fc1cc4c6b8f6c2d022 ;;
  *) echo "Models: tiny.en, base.en (default), small.en" >&2; exit 2 ;;
esac

dir="${XDG_DATA_HOME:-$HOME/.local/share}/kompact/whisper"
file="$dir/ggml-$model.bin"
url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$model.bin"

if [[ -f "$file" ]] && echo "$sha  $file" | sha1sum -c --quiet - 2>/dev/null; then
  echo "Already here: $file"
else
  mkdir -p "$dir"
  echo "Fetching $url"
  curl -fL --progress-bar -o "$file.part" "$url"
  if ! echo "$sha  $file.part" | sha1sum -c --quiet -; then
    rm -f "$file.part"
    echo "The download doesn't match whisper.cpp's checksum; nothing kept." >&2
    exit 1
  fi
  mv "$file.part" "$file"
  echo "Saved $file"
fi

command -v whisper-cli >/dev/null || echo "Now: sudo apt install whisper.cpp"

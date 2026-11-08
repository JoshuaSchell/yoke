#!/usr/bin/env bash
# Renders the screenshots in shots/ headlessly from the demo scenes (src/app.rs, `scenes`).
# Each runs with a scratch HOME holding only a theme and appearance, so none of your
# settings, projects or threads show. macOS (the scratch prefs go where macOS keeps them).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q
bin=$PWD/target/debug/yoke
# Labels for the picker, as the CLI reports them, so it doesn't fall back to bare names.
models='[["Default","Default"],["opus","Opus 5.5"],["fable","Fable 5.1"],["sonnet","Sonnet 5.5"],["haiku","Haiku 5.5"],["claude-opus-5","Opus 5"],["claude-sonnet-5","Sonnet 5"]]'
snap() { # name scene theme appearance
  local home; home=$(mktemp -d)
  local dir="$home/Library/Application Support/dev.yoke"
  mkdir -p "$dir"
  printf '{"theme":"%s","appearance":"%s","models.claude":%s}' "$3" "$4" "$models" > "$dir/prefs.json"
  # Keeps finding the CLIs (detection also looks under HOME) without your logins.
  HOME="$home" PATH="$PATH:$(dirname "$(command -v claude || echo /usr/bin/true)")" \
    "$bin" --snap "shots/$1.png" --scene "$2" --size 1180x780 >/dev/null 2>&1
  rm -rf "$home"
  echo "shots/$1.png"
}
snap demo-dark demo OpenCode Dark
snap demo-light demo OpenCode Light
snap empty-dark empty OpenCode Dark
snap empty-light empty OpenCode Light
snap projects-dark projects OpenCode Dark
snap long-dark long OpenCode Dark
snap long-light long OpenCode Light
snap picker-dark picker OpenCode Dark
snap picker-light picker OpenCode Light
snap effort-dark effort OpenCode Dark
snap settings-dark settings OpenCode Dark
snap usage-dark usage OpenCode Dark
snap usage-light usage OpenCode Light
snap nord-dark demo Nord Dark
snap gruvbox-light demo Gruvbox Light

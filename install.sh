#!/usr/bin/env bash
# cswap installer — https://github.com/sidhanthapoddar99/claude-swap-rs
#
#   curl -fsSL https://raw.githubusercontent.com/sidhanthapoddar99/claude-swap-rs/main/install.sh | bash
#
# Flags:
#   --yes            assume yes for the shell-rc prompt (scripted installs)
#   --no-modify-rc   never touch rc files, just print instructions
#   --dir <path>     install directory (default ~/.local/bin)
set -euo pipefail

REPO="sidhanthapoddar99/claude-swap-rs"
INSTALL_DIR="${CSWAP_INSTALL_DIR:-$HOME/.local/bin}"
MODIFY_RC="ask"

while [ $# -gt 0 ]; do
  case "$1" in
    --yes) MODIFY_RC="yes" ;;
    --no-modify-rc) MODIFY_RC="no" ;;
    --dir) shift; INSTALL_DIR="$1" ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
  shift
done

# --- detect platform -------------------------------------------------------
os="$(uname -s)"; arch="$(uname -m)"
case "$os/$arch" in
  Linux/x86_64)   target="x86_64-unknown-linux-musl" ;;
  Linux/aarch64)  target="aarch64-unknown-linux-musl" ;;
  Darwin/x86_64)  target="x86_64-apple-darwin" ;;
  Darwin/arm64)   target="aarch64-apple-darwin" ;;
  *) echo "unsupported platform: $os/$arch" >&2; exit 1 ;;
esac

# --- download latest release ----------------------------------------------
asset="cswap-${target}.tar.gz"
url="https://github.com/${REPO}/releases/latest/download/${asset}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading ${asset} (latest release)..."
curl -fsSL -o "$tmp/$asset" "$url"
curl -fsSL -o "$tmp/$asset.sha256" "$url.sha256"

echo "Verifying checksum..."
expected="$(awk '{print $1}' "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
fi
if [ "$expected" != "$actual" ]; then
  echo "checksum mismatch — aborting (expected $expected, got $actual)" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tar -xzf "$tmp/$asset" -C "$tmp"
install -m 755 "$tmp/cswap" "$INSTALL_DIR/cswap"
echo "Installed: $INSTALL_DIR/cswap ($("$INSTALL_DIR/cswap" --version 2>/dev/null || echo 'installed'))"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "NOTE: $INSTALL_DIR is not on your PATH — add it to your shell rc." ;;
esac

# --- shell integration (ask, then append — idempotent) ---------------------
MARKER="# >>> cswap shell integration >>>"

append_block() { # $1 = rc file, $2 = shell name
  if [ -f "$1" ] && grep -qF "$MARKER" "$1"; then
    echo "  $1: already set up, skipping."
    return
  fi
  {
    printf '\n%s\n' "$MARKER"
    printf 'eval "$(cswap shell-init %s)"\n' "$2"
    printf '# <<< cswap shell integration <<<\n'
  } >> "$1"
  echo "  $1: added."
}

# Every rc whose shell exists on this machine gets an offer; the user's own
# $SHELL rc is offered even if the file doesn't exist yet.
candidates=""
[ -f "$HOME/.zshrc" ] || case "${SHELL:-}" in */zsh) : ;; *) false ;; esac && candidates="$candidates zsh:$HOME/.zshrc"
[ -f "$HOME/.bashrc" ] || case "${SHELL:-}" in */bash) : ;; *) false ;; esac && candidates="$candidates bash:$HOME/.bashrc"

if [ -z "$candidates" ]; then
  MODIFY_RC="no"
fi

if [ "$MODIFY_RC" = "ask" ]; then
  # Under `curl | bash` stdin is the pipe; prompt via the terminal directly.
  if [ -r /dev/tty ]; then
    printf 'Add cswap shell integration (activate + claude wrapper) to:%s? [Y/n] ' \
      "$(echo "$candidates" | sed 's/[a-z]*:/ /g')"
    read -r reply < /dev/tty || reply=""
    case "$reply" in [Nn]*) MODIFY_RC="no" ;; *) MODIFY_RC="yes" ;; esac
  else
    MODIFY_RC="no"
  fi
fi

if [ "$MODIFY_RC" = "yes" ]; then
  echo "Setting up shell integration:"
  for entry in $candidates; do
    shell="${entry%%:*}"; rc="${entry#*:}"
    append_block "$rc" "$shell"
  done
  echo "Open a new terminal (or 'source' your rc) to pick it up."
else
  echo
  echo "Shell integration not installed. To enable 'cswap activate' and the"
  echo "claude wrapper, add this to your ~/.zshrc or ~/.bashrc:"
  echo '  eval "$(cswap shell-init zsh)"   # or: bash'
fi

echo
echo "Next: log into Claude Code ('claude'), then run: cswap login"

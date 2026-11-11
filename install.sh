#!/usr/bin/env bash
# cswap installer for Linux and macOS: https://github.com/sidhanthapoddar99/claude-swap-rs
# Windows: use install.ps1 from the same place.
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
  Darwin/aarch64) target="aarch64-apple-darwin" ;;
  Linux/arm64)    target="aarch64-unknown-linux-musl" ;;
  *) echo "There is no prebuilt cswap for $os/$arch. Build it with: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
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
  echo "The download does not match its checksum (expected $expected, got $actual). Nothing was installed." >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tar -xzf "$tmp/$asset" -C "$tmp"
install -m 755 "$tmp/cswap" "$INSTALL_DIR/cswap"
echo "Installed $INSTALL_DIR/cswap ($("$INSTALL_DIR/cswap" --version 2>/dev/null || echo 'version unknown'))"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "NOTE: $INSTALL_DIR is not on your PATH. Add it in your shell's rc file." ;;
esac

# --- shell integration (ask, then append; safe to run twice) ---------------
MARKER="# >>> cswap shell integration >>>"

append_block() { # $1 = rc file, $2 = shell name
  if [ -f "$1" ] && grep -qF "cswap shell-init" "$1"; then
    echo "  $1: already set up, skipping."
    return
  fi
  mkdir -p "$(dirname "$1")"
  {
    printf '\n%s\n' "$MARKER"
    if [ "$2" = "fish" ]; then
      printf 'cswap shell-init fish | source\n'
    else
      printf 'eval "$(cswap shell-init %s)"\n' "$2"
    fi
    printf '# <<< cswap shell integration <<<\n'
  } >> "$1"
  echo "  $1: added."
}

# A shell gets an offer when its rc file exists, or when it is your $SHELL.
candidates=""
offer() { # $1 = shell name, $2 = rc file
  if [ -f "$2" ] || [ "$(basename "${SHELL:-}")" = "$1" ]; then
    candidates="$candidates $1:$2"
  fi
}
offer zsh "$HOME/.zshrc"
offer bash "$HOME/.bashrc"
offer fish "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"

if [ -z "$candidates" ]; then
  MODIFY_RC="no"
fi

if [ "$MODIFY_RC" = "ask" ]; then
  # Under `curl | bash` stdin is the pipe, so ask on the terminal itself.
  if [ -r /dev/tty ]; then
    echo "Shell integration makes 'claude', 'codex', 'agy' and 'grok' run through cswap."
    printf 'Add it to:%s? [Y/n] ' "$(echo "$candidates" | sed 's/[a-z]*:/ /g')"
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
  echo "Open a new terminal to pick it up."
else
  echo
  echo "Shell integration was not installed. Without it, start agents with"
  echo "'cswap claude', 'cswap codex' and so on. To make the plain commands go"
  echo "through cswap, add one line to your shell's rc file:"
  echo '  eval "$(cswap shell-init zsh)"        # zsh; use bash for bash'
  echo '  cswap shell-init fish | source        # fish'
fi

echo
echo "Next:"
echo "  cswap                   open the dashboard"
echo "  cswap add claude work   add a second Claude account named work"
echo "  cswap -h                every command"
if [ -d "$HOME/.local/share/cswap/profiles" ]; then
  echo "  cswap migrate           bring your cswap 0.6 profiles over"
fi

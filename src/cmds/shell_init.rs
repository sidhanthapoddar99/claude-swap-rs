//! `cswap shell-init <shell>` — emit the shell integration.
//!
//! Installs two functions in the user's shell:
//!   cswap()  — intercepts `cswap activate` so the export can land in THIS
//!              shell (a child process can't set parent env); everything else
//!              passes through to the real binary.
//!   claude() — routes bare `claude` through the hidden `_claude` shim, so it
//!              runs as $CSWAP_ACTIVE (or the default account). `command
//!              claude` always bypasses everything.

use anyhow::{bail, Result};

const POSIX_SNIPPET: &str = r#"# cswap shell integration (bash/zsh)
cswap() {
  if [ "$1" = "activate" ]; then
    local __cswap_out
    if __cswap_out="$(command cswap activate --print "${2:-}")"; then
      eval "$__cswap_out"
    else
      return 1
    fi
  else
    command cswap "$@"
  fi
}

claude() {
  command cswap _claude "$@"
}
"#;

pub fn run(shell: &str) -> Result<()> {
    match shell {
        "bash" | "zsh" => {
            print!("{POSIX_SNIPPET}");
            Ok(())
        }
        other => bail!("unsupported shell '{other}' (supported: bash, zsh)"),
    }
}

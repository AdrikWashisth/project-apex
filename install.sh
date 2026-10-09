#!/usr/bin/env bash
#
# One-shot APEX installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/AdrikWashisth/project-apex/main/install.sh | bash
#
# or, from a clone:
#
#   ./install.sh
#
# The script is idempotent: re-running it updates the binary without touching
# your configuration, agents or task history.

set -euo pipefail

REPO="https://github.com/AdrikWashisth/project-apex"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_BIN="${APEX_INSTALL_DIR:-$HOME/.local/bin}"

log() { printf '\033[36m[apex]\033[0m %s\n' "$*"; }
fail() { printf '\033[31m[apex]\033[0m %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- rust toolchain

ensure_rust() {
    if command -v cargo >/dev/null 2>&1; then
        log "cargo found: $(cargo --version)"
        return
    fi

    log "cargo not found; installing via rustup"
    export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
    export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

    curl --proto '=https' --tlsv1.2 -fsSf https://sh.rustup.rs -o /tmp/apex-rustup.sh
    sh /tmp/apex-rustup.sh -y --no-modify-path --profile default
    rm -f /tmp/apex-rustup.sh
    # shellcheck disable=SC1091
    . "$CARGO_HOME/env"
    log "cargo installed: $(cargo --version)"
}

ensure_cxx_toolchain() {
    if command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1; then
        return
    fi
    log "no C compiler found; attempting to install one (needed for bundled SQLite)"
    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get update -qq && sudo apt-get install -y --no-install-recommends gcc libc6-dev pkg-config
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y gcc gcc-c++ make pkgconfig
    elif command -v yum >/dev/null 2>&1; then
        sudo yum install -y gcc gcc-c++ make pkgconfig
    elif command -v pacman >/dev/null 2>&1; then
        sudo pacman -S --noconfirm base-devel
    elif command -v apk >/dev/null 2>&1; then
        sudo apk add --no-cache build-base
    else
        fail "could not find a supported package manager; install a C compiler and re-run"
    fi
}

ensure_git() {
    command -v git >/dev/null 2>&1 || fail "git is required but was not found"
}

# ---------------------------------------------------------------- build

build_release() {
    log "building the release binary (this takes a few minutes on a cold cache)"
    (cd "$CRATE_DIR" && cargo build --release --bin apex)
    mkdir -p "$INSTALL_BIN"
    cp -f "$CRATE_DIR/target/release/apex" "$INSTALL_BIN/apex"
    chmod +x "$INSTALL_BIN/apex"
}

# ---------------------------------------------------------------- PATH

ensure_path() {
    case ":$PATH:" in
        *":$INSTALL_BIN:"*) return ;;
    esac

    local shell_rc
    shell_rc="$(detect_shell_rc)"
    log "adding $INSTALL_BIN to PATH in $shell_rc"
    {
        printf '\n# APEX\n'
        printf 'export PATH="%s:$PATH"\n' "$INSTALL_BIN"
    } >>"$shell_rc"
    export PATH="$INSTALL_BIN:$PATH"
}

detect_shell_rc() {
    case "${SHELL:-}" in
        */zsh) printf '%s' "${ZDOTDIR:-$HOME}/.zshrc" ;;
        */bash) printf '%s' "$HOME/.bashrc" ;;
        *) printf '%s' "$HOME/.profile" ;;
    esac
}

# ---------------------------------------------------------------- main

main() {
    if [[ ! -f "$CRATE_DIR/Cargo.toml" ]]; then
        fail "run this from the repository, or via the curl one-liner"
    fi

    ensure_rust
    ensure_cxx_toolchain
    ensure_git
    build_release
    ensure_path

    log "installed to $INSTALL_BIN/apex"
    "$INSTALL_BIN/apex" --version

    cat <<'NEXT'

APEX is installed. Next steps:

  1. Open a terminal so PATH takes effect (or run: source ~/.bashrc).
  2. Check the environment:      apex doctor
  3. Point it at a model:         apex models set openai/gpt-4o-mini
     (then: export OPENAI_API_KEY=sk-...)
  4. Run a task against a repo:   cd ~/code/my-project
                                  apex run "Add input validation to user creation"

See docs/ in the repository for the architecture, roadmap and security model.
NEXT
}

main "$@"

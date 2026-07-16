#!/usr/bin/env bash

# Keyless one-line installer for the *-tron CLIs (forge-tron, cast-tron,
# anvil-tron, chisel-tron).
#
# It detects the host OS/arch, downloads the matching archive from the rolling
# `foundry-tron-latest` prerelease of elsvv/foundry-tron, verifies its SHA-256
# checksum when the release publishes one, unpacks the four binaries into
# ~/.foundry-tron/bin, and prints a PATH hint. Downloads go over plain curl and
# fall back to `gh release download` when the asset host is unreachable (corp or
# regional blocks). Re-runs are idempotent.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/elsvv/foundry-tron/master/install-foundry-tron.sh | bash
#
# Flags:
#   --modify-path        Append the PATH export to your shell profile.
#   --dir <path>         Install into <path> instead of ~/.foundry-tron.
#   --tag <tag>          Pull from a different release tag.
#   --require-checksum   Fail if the release has no checksum to verify against.
#   -h, --help           Show this help.

set -euo pipefail

REPO="elsvv/foundry-tron"
RELEASE_TAG="foundry-tron-latest"
BINARIES="forge-tron cast-tron anvil-tron chisel-tron"

MODIFY_PATH=0
REQUIRE_CHECKSUM=0
INSTALL_ROOT="${FOUNDRY_TRON_DIR:-$HOME/.foundry-tron}"

say() { printf 'install-foundry-tron: %s\n' "$1" >&2; }
warn() { printf 'install-foundry-tron: warning: %s\n' "$1" >&2; }
err() {
  printf 'install-foundry-tron: error: %s\n' "$1" >&2
  exit 1
}

usage() {
  sed -n '3,26p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
  exit 0
}

need() {
  command -v "$1" >/dev/null 2>&1 || err "required command not found: $1"
}

# Print the release asset base name for this host, e.g. foundry-tron_darwin_arm64.
detect_asset_base() {
  local os arch
  case "$(uname -s)" in
    Darwin) os="darwin" ;;
    Linux) os="linux" ;;
    *) err "unsupported OS: $(uname -s) (use install-foundry-tron.ps1 on Windows)" ;;
  esac
  case "$(uname -m)" in
    arm64 | aarch64) arch="arm64" ;;
    x86_64 | amd64) arch="amd64" ;;
    *) err "unsupported architecture: $(uname -m)" ;;
  esac
  printf 'foundry-tron_%s_%s' "$os" "$arch"
}

# Download <url> to <dest> over plain curl. Returns non-zero on failure.
curl_download() {
  local url="$1" dest="$2"
  curl -fsSL --retry 3 --retry-delay 2 -o "$dest" "$url"
}

# Download release asset <name> into directory <dir> via authenticated gh.
gh_download() {
  local name="$1" dir="$2"
  command -v gh >/dev/null 2>&1 || return 1
  gh release download "$RELEASE_TAG" \
    --repo "$REPO" \
    --pattern "$name" \
    --dir "$dir" \
    --clobber >/dev/null 2>&1
}

# Fetch release asset <name> to <dest>: plain curl first, gh fallback second.
fetch_asset() {
  local name="$1" dest="$2"
  local url="https://github.com/$REPO/releases/download/$RELEASE_TAG/$name"
  if curl_download "$url" "$dest"; then
    return 0
  fi
  say "curl could not reach the asset host; falling back to 'gh release download'"
  local dir
  dir="$(dirname "$dest")"
  if gh_download "$name" "$dir"; then
    [ "$dir/$name" = "$dest" ] || mv -f "$dir/$name" "$dest"
    return 0
  fi
  return 1
}

# Compute the SHA-256 of <file> as a bare lowercase hex string.
sha256_of() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    err "no sha256sum or shasum available to verify the download"
  fi
}

# Verify <archive> against its published <asset>.sha256, when one exists.
verify_checksum() {
  local archive="$1" asset="$2" tmp="$3"
  local sumfile="$tmp/$asset.sha256"
  if ! fetch_asset "$asset.sha256" "$sumfile"; then
    if [ "$REQUIRE_CHECKSUM" -eq 1 ]; then
      err "no checksum published for $asset and --require-checksum was set"
    fi
    warn "no checksum published for $asset; skipping integrity verification"
    return 0
  fi
  local expected actual
  expected="$(awk '{print $1}' "$sumfile")"
  actual="$(sha256_of "$archive")"
  if [ -z "$expected" ]; then
    err "checksum file for $asset was empty"
  fi
  if [ "$expected" != "$actual" ]; then
    err "checksum mismatch for $asset (expected $expected, got $actual)"
  fi
  say "checksum verified ($actual)"
}

# Append the PATH export to the user's shell profile, once.
modify_profile() {
  local bin_dir="$1" profile pref_shell line
  case "${SHELL:-}" in
    */zsh)
      profile="${ZDOTDIR:-$HOME}/.zshenv"
      pref_shell="zsh"
      ;;
    */bash)
      profile="$HOME/.bashrc"
      pref_shell="bash"
      ;;
    */fish)
      profile="$HOME/.config/fish/config.fish"
      pref_shell="fish"
      ;;
    *)
      warn "could not detect your shell; add $bin_dir to your PATH manually"
      return 0
      ;;
  esac
  if [ "$pref_shell" = "fish" ]; then
    line="fish_add_path -a \"$bin_dir\""
  else
    line="export PATH=\"\$PATH:$bin_dir\""
  fi
  mkdir -p "$(dirname "$profile")"
  if [ -f "$profile" ] && grep -qF "$bin_dir" "$profile"; then
    say "$bin_dir already present in $profile"
  else
    printf '\n%s\n' "$line" >> "$profile"
    say "added $bin_dir to PATH in $profile ($pref_shell)"
    say "run 'source $profile' or open a new terminal to pick it up"
  fi
}

main() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --modify-path) MODIFY_PATH=1 ;;
      --require-checksum) REQUIRE_CHECKSUM=1 ;;
      --dir)
        [ "$#" -ge 2 ] || err "--dir requires a path argument"
        INSTALL_ROOT="$2"
        shift
        ;;
      --tag)
        [ "$#" -ge 2 ] || err "--tag requires a value"
        RELEASE_TAG="$2"
        shift
        ;;
      -h | --help) usage ;;
      *) err "unknown argument: $1 (try --help)" ;;
    esac
    shift
  done

  need curl
  need tar

  local asset archive bin_dir tmp
  asset="$(detect_asset_base).tar.gz"
  bin_dir="$INSTALL_ROOT/bin"
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/foundry-tron.XXXXXX")"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT

  say "installing the *-tron CLIs from $REPO@$RELEASE_TAG"
  say "target: $bin_dir"

  archive="$tmp/$asset"
  say "downloading $asset"
  fetch_asset "$asset" "$archive" || err "could not download $asset (checked curl and gh)"

  verify_checksum "$archive" "$asset" "$tmp"

  local stage="$tmp/stage"
  mkdir -p "$stage"
  tar -xzf "$archive" -C "$stage"

  local b
  for b in $BINARIES; do
    [ -f "$stage/$b" ] || err "archive is missing $b"
    chmod +x "$stage/$b"
  done

  mkdir -p "$bin_dir"
  for b in $BINARIES; do
    mv -f "$stage/$b" "$bin_dir/$b"
  done
  say "installed: $BINARIES"

  # Best-effort smoke check so a broken download surfaces immediately.
  if "$bin_dir/forge-tron" --version >/dev/null 2>&1; then
    say "forge-tron reports: $("$bin_dir/forge-tron" --version 2>/dev/null | head -n1)"
  else
    warn "forge-tron --version did not run cleanly; check your platform"
  fi

  if [ "$MODIFY_PATH" -eq 1 ]; then
    modify_profile "$bin_dir"
  elif [ -t 0 ] && [ -t 1 ]; then
    printf 'install-foundry-tron: add %s to your PATH now? [y/N] ' "$bin_dir" >&2
    local reply
    read -r reply
    case "$reply" in
      [yY] | [yY][eE][sS]) modify_profile "$bin_dir" ;;
      *) say "leaving your PATH untouched" ;;
    esac
  fi

  case ":$PATH:" in
    *":$bin_dir:"*) say "$bin_dir is already on your PATH; you're ready to go" ;;
    *)
      say "add $bin_dir to your PATH to use the CLIs, e.g.:"
      # `$PATH` is meant to be printed literally for the user to copy.
      # shellcheck disable=SC2016
      printf '\n  export PATH="%s:$PATH"\n\n' "$bin_dir" >&2
      say "or re-run with --modify-path to append it to your shell profile"
      ;;
  esac

  say "done. try: forge-tron --version"
}

main "$@"

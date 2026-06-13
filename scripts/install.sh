#!/usr/bin/env sh
# Tyra language installer
# Usage: curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
#        curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh -s -- --version v0.11.0
#        sh install.sh --prefix /usr/local

set -eu

REPO="tyra-lang/tyra"
DEFAULT_PREFIX="${HOME}/.local"
TYRA_VERSION="${TYRA_VERSION:-}"

# ---------- argument parsing ----------
PREFIX=""
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix)
      shift
      PREFIX="$1"
      ;;
    --prefix=*)
      PREFIX="${1#--prefix=}"
      ;;
    --version)
      shift
      TYRA_VERSION="$1"
      ;;
    --version=*)
      TYRA_VERSION="${1#--version=}"
      ;;
    -h|--help)
      echo "Usage: install.sh [--prefix DIR] [--version TAG]"
      echo "  --prefix DIR   Install to DIR/bin and DIR/lib/tyra (default: \$HOME/.local)"
      echo "  --version TAG  Install a specific release tag (default: latest)"
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
  shift
done

PREFIX="${PREFIX:-$DEFAULT_PREFIX}"

# ---------- helpers ----------
info()  { printf '\033[1;32m==> \033[0m%s\n' "$*"; }
warn()  { printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || error "Required command not found: $1"
}

# ---------- platform detection ----------
detect_platform() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin)
      case "$arch" in
        arm64) echo "macos-arm64" ;;
        x86_64) error "macOS Intel (x86_64) pre-built binaries are not available. Build from source: https://github.com/${REPO}/blob/main/docs/getting-started/01-installation.md#build-from-source" ;;
        *) error "Unsupported macOS architecture: $arch" ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64)
          # Detect musl libc (Alpine and other musl-based distros)
          if [ -f /etc/alpine-release ] || (ldd --version 2>&1 | grep -qi musl); then
            echo "linux-musl-x86_64-static"
          else
            echo "linux-x86_64"
          fi
          ;;
        aarch64|arm64)
          error "Linux ARM64 is not yet available. Use the source build or follow https://github.com/${REPO}/blob/main/docs/getting-started/01-installation.md"
          ;;
        *) error "Unsupported Linux architecture: $arch" ;;
      esac
      ;;
    *) error "Unsupported OS: $os" ;;
  esac
}

# ---------- version resolution ----------
resolve_version() {
  need_cmd curl
  info "Resolving latest release..."
  tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | head -1 \
    | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
  [ -n "$tag" ] || error "Could not resolve latest release. Set TYRA_VERSION to override."
  echo "$tag"
}

# ---------- checksum verification ----------
verify_checksum() {
  archive="$1"
  expected_line="$2"   # "<sha256>  <filename>" line from SHA256SUMS

  if command -v sha256sum >/dev/null 2>&1; then
    echo "$expected_line" | sha256sum --check --status \
      || error "SHA256 checksum mismatch for $(basename "$archive")"
  elif command -v shasum >/dev/null 2>&1; then
    echo "$expected_line" | shasum -a 256 --check --status \
      || error "SHA256 checksum mismatch for $(basename "$archive")"
  else
    warn "No sha256sum or shasum found — skipping checksum verification"
  fi
}

# ---------- main ----------
need_cmd curl
need_cmd tar

platform="$(detect_platform)"
info "Platform: $platform"

if [ -z "$TYRA_VERSION" ]; then
  TYRA_VERSION="$(resolve_version)"
fi
info "Version: $TYRA_VERSION"

archive_name="tyra-${TYRA_VERSION}-${platform}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/${TYRA_VERSION}"
archive_url="${base_url}/${archive_name}"
sums_url="${base_url}/SHA256SUMS"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

info "Downloading $archive_name..."
curl -fsSL --progress-bar -o "${tmpdir}/${archive_name}" "$archive_url" \
  || error "Download failed. Check that release ${TYRA_VERSION} exists: https://github.com/${REPO}/releases"

info "Verifying checksum..."
sums_file="${tmpdir}/SHA256SUMS"
if curl -fsSL -o "$sums_file" "$sums_url" 2>/dev/null; then
  expected_line="$(grep "${archive_name}" "$sums_file" || true)"
  if [ -n "$expected_line" ]; then
    # sha256sum / shasum expect the file to be in the current directory
    ( cd "$tmpdir" && verify_checksum "$archive_name" "$expected_line" )
    info "Checksum OK"
  else
    warn "Archive not found in SHA256SUMS — skipping verification"
  fi
else
  warn "Could not download SHA256SUMS — skipping verification"
fi

info "Extracting..."
tar -xzf "${tmpdir}/${archive_name}" -C "$tmpdir"

# The archive contains a single top-level directory
extracted_dir="$(find "$tmpdir" -maxdepth 1 -mindepth 1 -type d | head -1)"
[ -n "$extracted_dir" ] || error "Archive appears empty"

# ---------- install ----------
bin_dir="${PREFIX}/bin"
lib_dir="${PREFIX}/lib/tyra"

mkdir -p "$bin_dir" "$lib_dir"

info "Installing binary to ${bin_dir}/tyra..."
install -m 755 "${extracted_dir}/tyra" "${bin_dir}/tyra"

info "Installing runtime library to ${lib_dir}/libtyra_runtime.a..."
install -m 644 "${extracted_dir}/libtyra_runtime.a" "${lib_dir}/libtyra_runtime.a"

info "Installing stdlib to ${lib_dir}/stdlib/..."
rm -rf "${lib_dir}/stdlib"
cp -r "${extracted_dir}/stdlib" "${lib_dir}/stdlib"

# ---------- PATH hint ----------
echo ""
info "Tyra ${TYRA_VERSION} installed to ${PREFIX}"

case ":${PATH}:" in
  *:"${bin_dir}":*) ;;
  *)
    echo ""
    echo "  ${bin_dir} is not in your PATH."
    echo "  Add this to your shell profile:"
    echo ""
    echo "    export PATH=\"${bin_dir}:\$PATH\""
    echo ""
    ;;
esac

echo "  Run: tyra --version"
echo ""

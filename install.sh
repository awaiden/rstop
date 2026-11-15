#!/usr/bin/env sh
set -eu

REPO="awaiden/rstop"
BIN_NAME="rstop"
INSTALL_DIR="${RSTOP_INSTALL_DIR:-/usr/local/bin}"

OS="$(uname -s)"
if [ "$OS" != "Linux" ]; then
  echo "error: $BIN_NAME only supports Linux (detected: $OS)" >&2
  exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64 | amd64)
    TARGET="x86_64-unknown-linux-musl"
    ;;
  aarch64 | arm64)
    TARGET="aarch64-unknown-linux-gnu"
    ;;
  *)
    echo "error: unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

if [ -n "${RSTOP_VERSION:-}" ]; then
  TAG="$RSTOP_VERSION"
else
  TAG="$(
    curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
      | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
      | head -n 1
  )"
  if [ -z "$TAG" ]; then
    echo "error: failed to determine latest release tag from GitHub" >&2
    exit 1
  fi
fi

ARCHIVE="${BIN_NAME}-${TAG}-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/$TAG"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

echo "Downloading $BIN_NAME $TAG ($TARGET)..."
curl -fsSL "$BASE_URL/$ARCHIVE" -o "$TMP_DIR/$ARCHIVE"

if command -v sha256sum >/dev/null 2>&1; then
  curl -fsSL "$BASE_URL/$ARCHIVE.sha256" -o "$TMP_DIR/$ARCHIVE.sha256"
  (cd "$TMP_DIR" && sha256sum -c "$ARCHIVE.sha256")
elif command -v shasum >/dev/null 2>&1; then
  curl -fsSL "$BASE_URL/$ARCHIVE.sha256" -o "$TMP_DIR/$ARCHIVE.sha256"
  (cd "$TMP_DIR" && shasum -a 256 -c "$ARCHIVE.sha256")
else
  echo "error: cannot verify checksum (neither sha256sum nor shasum is installed)" >&2
  exit 1
fi

tar -C "$TMP_DIR" -xzf "$TMP_DIR/$ARCHIVE"

probe="$INSTALL_DIR"
while [ ! -e "$probe" ]; do
  parent="$(dirname "$probe")"
  if [ "$parent" = "$probe" ]; then
    break
  fi
  probe="$parent"
done

if [ -d "$probe" ] && [ -w "$probe" ]; then
  mkdir -p "$INSTALL_DIR"
  install -m 755 "$TMP_DIR/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
else
  sudo mkdir -p "$INSTALL_DIR"
  sudo install -m 755 "$TMP_DIR/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
fi

echo "Installed $BIN_NAME $TAG to $INSTALL_DIR/$BIN_NAME"

#!/usr/bin/env bash
# Builds Scrimp's installer and puts it in installers/ under a name with no
# version, so "the latest installer" is always the same address on a
# release. Used by both CI and the Release workflow, so they can't build
# differently. Runs on Windows, in Git Bash.
#
#   scripts/build-installer.sh
#
# Set SCRIMP_VERSION to have it refuse a build that says it is anything else.

set -euo pipefail

version="${SCRIMP_VERSION:-$(cargo xtask version)}"
cargo build --release --locked -p scrimp-cli -p scrimpw -p scrimp-shell

stage=target/installer-stage
rm -rf "$stage"
mkdir -p "$stage" installers
cp target/release/scrimp.exe target/release/scrimpw.exe target/release/scrimp_shell.dll "$stage/"

said=$("$stage/scrimp.exe" --version)
if [ "$said" != "scrimp $version" ]; then
  echo "::error::Expected \"scrimp $version\" but the build says \"$said\"." >&2
  exit 1
fi

# NSIS: on the PATH, where its installer puts it, or installed now.
makensis=$(command -v makensis || true)
if [ -z "$makensis" ] && [ -x "/c/Program Files (x86)/NSIS/makensis.exe" ]; then
  makensis="/c/Program Files (x86)/NSIS/makensis.exe"
fi
if [ -z "$makensis" ]; then
  choco install nsis --yes --no-progress >/dev/null
  makensis="/c/Program Files (x86)/NSIS/makensis.exe"
fi

"$makensis" -V2 \
  -DVERSION="$version" \
  -DSTAGE="$(cygpath -w "$PWD/$stage")" \
  -DOUT="$(cygpath -w "$PWD/installers/Scrimp-windows-x64-setup.exe")" \
  installer/scrimp.nsi
ls -la installers

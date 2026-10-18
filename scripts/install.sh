#!/bin/sh
# Install Brigadier on macOS from its GitHub release. Safe to run through `curl | sh`.
#
# BRIGADIER_INSTALL_DIR installs somewhere other than /Applications (tests use a scratch folder).
# BRIGADIER_RELEASE_URL reads the release files from elsewhere (tests serve them locally).
set -eu

version=0.1.0-alpha
team=7JQSPMWT79

fail() { printf 'install: %s\n' "$*" >&2; exit 1; }

cleanup() {
  if [ -n "$mounted" ]; then
    hdiutil detach "$mounted" -quiet || printf 'Could not eject %s; eject it in Finder.\n' "$mounted" >&2
  fi
  # scratch is a private directory created by mktemp, never a caller-supplied path.
  find "$scratch" -depth -delete
}

install_brigadier() {
  [ "$(uname -s)" = Darwin ] || fail 'Brigadier requires macOS.'
  major=$(sw_vers -productVersion | cut -d . -f 1)
  [ "$major" -ge 14 ] || fail 'macOS 14 or later is required.'
  release=${BRIGADIER_RELEASE_URL:-https://github.com/syncra-studio/brigadier-ai/releases/download/v$version}
  apps=${BRIGADIER_INSTALL_DIR:-/Applications}
  target="$apps/Brigadier.app"
  # One build runs on Apple silicon and Intel.
  asset="Brigadier-$version-universal.dmg"
  scratch=$(mktemp -d /tmp/brigadier-install.XXXXXX)
  mounted=''
  trap cleanup 0
  trap 'exit 130' HUP INT TERM

  printf 'Downloading Brigadier %s…\n' "$version"
  curl -fsSL --retry 3 "$release/SHA256SUMS" -o "$scratch/SHA256SUMS"
  checksum=$(awk -v asset="$asset" '$2 == asset { print $1 }' "$scratch/SHA256SUMS")
  printf '%s\n' "$checksum" | LC_ALL=C grep -Eq '^[0-9a-fA-F]{64}$' \
    || fail "No valid checksum for $asset."
  curl -fSL --retry 3 --progress-bar "$release/$asset" -o "$scratch/$asset"
  (cd "$scratch" && printf '%s  %s\n' "$checksum" "$asset" | shasum -a 256 -c - >/dev/null) \
    || fail 'The download is damaged (checksum mismatch); nothing was installed.'

  mkdir "$scratch/mount"
  hdiutil attach "$scratch/$asset" -readonly -nobrowse -mountpoint "$scratch/mount" -quiet
  mounted="$scratch/mount"
  source="$mounted/Brigadier.app"
  [ -d "$source" ] && [ ! -L "$source" ] || fail 'The download does not contain Brigadier.app.'
  # Signed by Syncra's Developer ID and notarized by Apple, or not installed.
  codesign --verify --deep --strict "$source" || fail 'The app signature is invalid.'
  codesign -dv "$source" 2>&1 | grep -qx "TeamIdentifier=$team" \
    || fail 'The app is not signed by the Brigadier developer.'
  spctl --assess --type execute "$source" 2>/dev/null \
    || fail 'macOS does not accept this app (not notarized); nothing was installed.'

  [ -d "$apps" ] || fail "$apps does not exist."
  [ ! -L "$target" ] || fail "$target is a symlink; install by hand."
  if [ -e "$target" ]; then
    printf 'Quit Brigadier and let its running sessions finish before replacing it.\n'
    printf 'Replace %s? [y/N] ' "$target"
    answer=''
    if ! read -r answer </dev/tty; then fail 'A terminal is needed to confirm the replacement.'; fi
    case "$answer" in y|Y|yes|YES) ;; *) printf 'Nothing was installed.\n'; return ;; esac
  fi
  if pgrep -f "^$target/Contents/MacOS/(brigadier|brigadierd)( |$)" >/dev/null; then
    fail 'Brigadier is still running. Quit it first.'
  fi

  # Administrator permission only when the folder needs it.
  elevated=''
  if [ ! -w "$apps" ] || { [ -e "$target" ] && [ ! -w "$target" ]; }; then
    printf 'Administrator permission is needed to install in %s.\n' "$apps"
    sudo -v
    elevated=sudo
  fi
  if [ -e "$target" ]; then
    $elevated rm -rf "$target"
  fi
  $elevated ditto "$source" "$target"
  printf 'Installed %s. Open it to begin; it updates itself from now on.\n' "$target"
}

install_brigadier

#!/bin/sh
# Install the macOS alpha from its GitHub release. Safe to run through `curl | sh`.
set -eu

fail() { printf 'install: %s\n' "$*" >&2; exit 1; }

cleanup() {
  if [ -n "$mounted" ]; then
    hdiutil detach "$mounted" -quiet || printf 'Could not eject %s; eject it in Finder.\n' "$mounted" >&2
  fi
  # scratch is a private directory created by mktemp, never a caller-supplied path.
  find "$scratch" -depth -delete
}

install_alpha() {
  [ "$(uname -s)" = Darwin ] || fail 'This alpha requires macOS.'
  major=$(sw_vers -productVersion | cut -d . -f 1)
  [ "$major" -ge 14 ] || fail 'macOS 14 or later is required.'
  case "$(uname -m)" in
    arm64) arch=aarch64 ;;
    x86_64) arch=x64 ;;
    *) fail 'Unsupported processor architecture.' ;;
  esac
  release=https://github.com/syncra-studio/brigadier-ai/releases/download/v0.1.0-alpha
  asset="Brigadier_0.1.0_${arch}.dmg"
  scratch=$(mktemp -d /tmp/brigadier-alpha-install.XXXXXX)
  mounted=''
  trap cleanup 0
  trap 'exit 130' HUP INT TERM

  printf 'Downloading %s and SHA256SUMS from v0.1.0-alpha…\n' "$asset"
  curl -fsSL --retry 3 "$release/SHA256SUMS" -o "$scratch/SHA256SUMS"
  checksum=$(awk -v asset="$asset" '$2 == asset { print $1 }' "$scratch/SHA256SUMS")
  [ "${#checksum}" -eq 64 ] \
    || fail "No valid checksum for $asset; this release may not include your architecture."
  printf '%s\n' "$checksum" | LC_ALL=C grep -Eq '^[0-9a-fA-F]{64}$' \
    || fail "Invalid checksum for $asset."
  curl -fsSL --retry 3 "$release/$asset" -o "$scratch/$asset"
  (cd "$scratch" && printf '%s  %s\n' "$checksum" "$asset" | shasum -a 256 -c -) \
    || fail 'SHA-256 verification failed; nothing was installed.'

  mkdir "$scratch/mount"
  hdiutil attach "$scratch/$asset" -readonly -nobrowse -mountpoint "$scratch/mount" -quiet
  mounted="$scratch/mount"
  source="$mounted/Brigadier.app"
  [ -d "$source" ] && [ ! -L "$source" ] || fail 'The DMG does not contain Brigadier.app.'
  codesign --verify --deep --strict "$source" || fail 'The app signature is invalid.'

  [ ! -L /Applications/Brigadier.app ] || fail '/Applications/Brigadier.app is a symlink; install manually.'
  if [ -e /Applications/Brigadier.app ]; then
    printf 'Quit Brigadier and let its active sessions finish before replacing it.\n'
    printf 'Replace /Applications/Brigadier.app? [y/N] '
    answer=''
    if ! read -r answer </dev/tty; then fail 'A terminal is required to confirm replacement.'; fi
    case "$answer" in y|Y|yes|YES) ;; *) printf 'Installation cancelled.\n'; return ;; esac
  fi
  if pgrep -f '^/Applications/Brigadier\.app/Contents/MacOS/(brigadier|brigadierd)( |$)' >/dev/null; then
    fail 'Brigadier is still running. Quit it before installing.'
  fi

  # Only elevate the copy/removal operations when /Applications needs it.
  elevated=''
  if [ ! -w /Applications ] || { [ -e /Applications/Brigadier.app ] && [ ! -w /Applications/Brigadier.app ]; }; then
    printf 'Administrator permission is needed to install in /Applications.\n'
    sudo -v
    elevated=sudo
  fi
  if [ -e /Applications/Brigadier.app ]; then
    $elevated rm -rf /Applications/Brigadier.app
  fi
  $elevated ditto "$source" /Applications/Brigadier.app
  printf 'This alpha is ad-hoc signed, without an Apple Developer ID or notarization.\n'
  printf 'Removing its download quarantine attribute lets this app open without Gatekeeper approval.\n'
  printf 'The checksum checks the release download; it does not establish developer identity or safety.\n'
  $elevated xattr -dr com.apple.quarantine /Applications/Brigadier.app
  printf 'Installed. Open /Applications/Brigadier.app to begin.\n'
}

install_alpha

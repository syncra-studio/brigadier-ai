# Install Brigadier alpha on macOS

The `v0.1.0-alpha` release requires macOS 14 or later. Download the DMG that
matches your Mac from the [alpha release](https://github.com/syncra-studio/brigadier-ai/releases/tag/v0.1.0-alpha):
`Brigadier_0.1.0_aarch64.dmg` for Apple Silicon. Intel availability is listed on
the release; the installer refuses to install an architecture missing from its checksums.

## One-line install

Once the alpha release is published:

```sh
curl -fsSL https://raw.githubusercontent.com/syncra-studio/brigadier-ai/v0.1.0-alpha/scripts/install.sh | sh
```

The script downloads the matching DMG and `SHA256SUMS` from that release,
checks SHA-256 and the app's ad-hoc signature, and copies the app into
`/Applications`. It asks before replacing an existing Brigadier and requires
you to quit it first. Let active agent sessions finish before updating.
Administrator permission is requested only when the copy requires it.

The installer removes `com.apple.quarantine` from **Brigadier.app** so this
unsigned alpha can open. This bypasses the download approval for this app;
it does not turn off Gatekeeper for other apps. To inspect the script first,
download it and read it before running `sh install.sh`.

## Manual install from the DMG

1. Download the DMG and `SHA256SUMS` into the same folder. In Terminal, enter
   that folder and check the hash (use the filename for your architecture):

   ```sh
   shasum -a 256 Brigadier_0.1.0_aarch64.dmg
   ```

   Compare all 64 characters with the matching line in `SHA256SUMS`. Stop if
   they differ. The checksum detects a changed or incomplete download; a
   checksum hosted alongside a file does not independently prove its origin.
2. Quit any existing Brigadier after its active sessions finish.
3. Open the DMG and drag **Brigadier.app** to **Applications**. Confirm any
   replacement intentionally, then eject the disk image.
4. Open Brigadier from Applications. If macOS blocks it, dismiss the warning
   and follow the approval steps below.

## Allow the unsigned alpha to open

After trying to open Brigadier, go to **System Settings › Privacy & Security**,
scroll to **Security**, find the blocked Brigadier entry, and choose
**Open Anyway**. On macOS 27 you may first see **Open** for the entry, then
**Open Anyway**. Authenticate and confirm opening when asked. The button is
available for about an hour after the blocked launch; try opening the app again
if it has disappeared.

Use this settings route on current macOS. Right-click or Control-click **Open**
no longer overrides Gatekeeper for software without a Developer ID signature or
notarization, starting with macOS 15. See [Apple's macOS 27 instructions](https://support.apple.com/en-gb/guide/mac-help/mh40617/27/mac/27)
and [Apple's Gatekeeper change](https://developer.apple.com/news/?id=saqachfa).

Alternatively, after verifying the download and copying it to Applications:

```sh
xattr -dr com.apple.quarantine /Applications/Brigadier.app
open /Applications/Brigadier.app
```

If permission is denied because an administrator owns the app, run the `xattr`
command with `sudo`. This removes quarantine recursively from this app and its
bundled executables. It grants no developer identity or Apple safety check.
Do not override warnings that say macOS detected malware.

## What “unsigned alpha” means

This build uses ad-hoc code signatures, with no Apple Developer ID signature
and no Apple notarization. Ad-hoc signatures allow the binaries to run on Apple
Silicon and detect changes to signed bundle contents; they do not identify the
publisher. Apple has not screened this build through its notarization service.
Alpha software can contain bugs. Use it on repositories you can recover through
version control and review the agent's work before publishing it.

Launch Brigadier and follow its setup to connect your installed agent CLIs.
macOS permissions for optional computer-use features are separate from
Gatekeeper approval. Ad-hoc builds may require those permissions again after
an update.

## Uninstall

Use **Uninstall Brigadier…** in the app's menu for the guided uninstaller. For
terminal use, see [`scripts/uninstall.sh`](../scripts/uninstall.sh): it supports
`--dry-run` and `--keep-data` and requires an explicit app identity. Preview:

```sh
bash scripts/uninstall.sh --app-id ai.brigadier.app --dry-run
```

Read its output before running without `--dry-run`; the default removes app
data. Removing only `/Applications/Brigadier.app` leaves your data in
`~/Library/Application Support/Brigadier`.

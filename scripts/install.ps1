# Install Brigadier on Windows from its GitHub release. Safe to run through `irm <url> | iex`.
#
# BRIGADIER_INSTALL_DIR installs somewhere other than %LOCALAPPDATA%\Brigadier (tests use a scratch folder).
# BRIGADIER_RELEASE_URL reads the release files from elsewhere (tests serve them locally).
#
# Everything runs in one script block, so a failure ends the install, not the PowerShell window.
& {
  $ErrorActionPreference = 'Stop'
  Set-StrictMode -Version 3
  # Invoke-WebRequest's progress display slows Windows PowerShell's downloads many times over.
  $ProgressPreference = 'SilentlyContinue'

  $version = '0.1.0-alpha'
  # The name in the Authenticode certificate the installer is signed with.
  $publisher = 'SYNCRA SRL'

  function Fail([string]$message) { throw "install: $message" }

  function Get-File([string]$url, [string]$path) {
    for ($try = 1; ; $try++) {
      try {
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $path
        return
      } catch {
        if ($try -ge 3) { throw }
        Start-Sleep -Seconds $try
      }
    }
  }

  if ([Environment]::OSVersion.Platform -ne 'Win32NT') { Fail 'This installer is for Windows; on macOS use install.sh.' }
  if ([Environment]::OSVersion.Version.Major -lt 10 -or -not [Environment]::Is64BitOperatingSystem) {
    Fail '64-bit Windows 10 or later is required.'
  }
  # Windows PowerShell 5.1 offers TLS 1.2 only when asked.
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

  $release = if ($env:BRIGADIER_RELEASE_URL) { $env:BRIGADIER_RELEASE_URL } else {
    "https://github.com/syncra-studio/brigadier-ai/releases/download/v$version"
  }
  # Where it goes: the folder asked for, else where Brigadier is installed already, else the
  # per-user default. The installer is told the same folder.
  $installed = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Brigadier' -ErrorAction SilentlyContinue
  $target = if ($env:BRIGADIER_INSTALL_DIR) { $env:BRIGADIER_INSTALL_DIR }
    elseif ($installed -and $installed.PSObject.Properties['InstallLocation']) { $installed.InstallLocation.Trim('"') }
    else { Join-Path $env:LOCALAPPDATA 'Brigadier' }
  $target = [IO.Path]::GetFullPath($target).TrimEnd('\')
  $asset = "Brigadier-$version-x64-setup.exe"
  $scratch = Join-Path ([IO.Path]::GetTempPath()) ("brigadier-install." + [IO.Path]::GetRandomFileName())
  New-Item -ItemType Directory -Path $scratch | Out-Null

  try {
    Write-Host "Downloading Brigadier $version..."
    Get-File "$release/SHA256SUMS" (Join-Path $scratch 'SHA256SUMS')
    $checksum = Get-Content (Join-Path $scratch 'SHA256SUMS') |
      ForEach-Object { $fields = -split $_; if ($fields.Count -eq 2 -and $fields[1].TrimStart('*') -eq $asset) { $fields[0] } } |
      Select-Object -First 1
    if (-not ($checksum -match '^[0-9a-fA-F]{64}$')) { Fail "No valid checksum for $asset." }
    $setup = Join-Path $scratch $asset
    Get-File "$release/$asset" $setup
    if ((Get-FileHash -Algorithm SHA256 -Path $setup).Hash -ne $checksum) {
      Fail 'The download is damaged (checksum mismatch); nothing was installed.'
    }

    # Signed by Brigadier's developer, or not installed.
    $signature = Get-AuthenticodeSignature -FilePath $setup
    # Unsigned is accepted, on the checksum alone, until Windows releases are signed.
    if ($signature.Status -eq 'NotSigned') {
      Write-Host 'The installer is not signed yet; it was verified by its checksum only.'
    } else {
      if ($signature.Status -ne 'Valid') { Fail 'The installer signature is invalid.' }
      if ($signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false) -ne $publisher) {
        Fail 'The installer is not signed by the Brigadier developer.'
      }
    }

    if (Test-Path -LiteralPath $target -PathType Leaf) { Fail "$target is a file, not a folder." }
    if ((Test-Path -LiteralPath (Join-Path $target 'brigadier-app.exe')) -or (Test-Path -LiteralPath (Join-Path $target 'brigadier.exe'))) {
      Write-Host 'Quit Brigadier and let its running sessions finish before replacing it.'
      try { $answer = Read-Host "Replace $target? [y/N]" } catch { Fail 'A terminal is needed to confirm the replacement.' }
      if ($answer -notmatch '^(y|yes)$') { Write-Host 'Nothing was installed.'; return }
    }
    $running = Get-Process -Name brigadier-app, brigadier, brigadierd -ErrorAction SilentlyContinue |
      Where-Object { $_.Path -and $_.Path.StartsWith("$target\", [StringComparison]::OrdinalIgnoreCase) }
    if ($running) { Fail 'Brigadier is still running. Quit it first.' }

    # The per-user installer needs no administrator permission. /D= comes last, unquoted.
    $run = Start-Process -FilePath $setup -ArgumentList '/S', "/D=$target" -Wait -PassThru
    if ($run.ExitCode -ne 0) { Fail "The installer failed (exit code $($run.ExitCode))." }
    if (-not (Test-Path -LiteralPath (Join-Path $target 'brigadier-app.exe'))) { Fail "The installer left no Brigadier in $target." }
    # The app with its daemon, its command and its computer-use helper (tauri.windows.conf.json).
    $missing = @('brigadierd.exe', 'brigadier.exe', 'brigadier-cu.exe' | Where-Object { -not (Test-Path -LiteralPath (Join-Path $target $_)) })
    if ($missing.Count -gt 0) { Fail "The installer left Brigadier incomplete in $target (no $($missing -join ', ')). Run it again." }
    Write-Host "Installed Brigadier in $target. Open it from the Start menu to begin; it updates itself from now on."
  } finally {
    # scratch is a private folder made above, never a caller-supplied path.
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
  }
}

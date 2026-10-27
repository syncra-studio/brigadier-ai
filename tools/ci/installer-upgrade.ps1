# Installs the Windows installer as a user does, then updates over that install as the updater
# does, once for each file the installer writes taken as the earlier install's app name. Every
# file must be there after each run: Tauri's installer deletes the earlier app's name after
# copying the new files, and an update over a build whose app was brigadier.exe deleted the
# brigadier command that way (installer-hooks.nsh). It installs Brigadier for the current user
# (its uninstall key included), so it runs on a CI runner only.
param([Parameter(Mandatory)] [string] $Setup)

$ErrorActionPreference = 'Stop'

if (-not $env:CI) { throw 'installer-upgrade.ps1 installs Brigadier for this user; it runs in CI only' }

# What the installer writes, from the configuration that decides it: the app (the [[bin]] of
# the desktop crate), its sidecars and the uninstaller.
$src = Join-Path $PSScriptRoot '../../apps/desktop/src-tauri'
$config = Get-Content -Raw (Join-Path $src 'tauri.windows.conf.json') | ConvertFrom-Json
$sidecars = @($config.bundle.externalBin | ForEach-Object { (Split-Path $_ -Leaf) + '.exe' })
if ((Get-Content -Raw (Join-Path $src 'Cargo.toml')) -notmatch '(?s)\[\[bin\]\]\s*name\s*=\s*"([^"]+)"') {
  throw 'no [[bin]] name in the desktop crate'
}
$files = @("$($Matches[1]).exe") + $sidecars + 'uninstall.exe'

$dir = Join-Path $env:RUNNER_TEMP 'brigadier-installed'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Brigadier'

function Install([string[]] $arguments, [string] $what) {
  $run = Start-Process -FilePath $Setup -ArgumentList $arguments -Wait -PassThru
  if ($run.ExitCode -ne 0) { throw "${what}: the installer exited with $($run.ExitCode)" }
  $missing = @($files | Where-Object { -not (Test-Path -LiteralPath (Join-Path $dir $_) -PathType Leaf) })
  if ($missing.Count -gt 0) { throw "${what}: the install has no $($missing -join ', ')" }
  Write-Host "ok  $what"
}

try {
  # /D names the folder, last and unquoted.
  Install @('/S', "/D=$dir") 'fresh install'
  foreach ($old in $files) {
    Set-ItemProperty -LiteralPath $key -Name MainBinaryName -Value $old
    # As the updater runs it, without /R: nothing opens the app.
    Install @('/S', '/UPDATE') "update over an install whose app was $old"
  }
  if ($sidecars -contains 'brigadier.exe') {
    & (Join-Path $dir 'brigadier.exe') --version
    if ($LASTEXITCODE -ne 0) { throw "the installed brigadier command exited with $LASTEXITCODE" }
  }
} finally {
  Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase) } |
    Stop-Process -Force
}

# Standalone Brigadier uninstaller for Windows, the counterpart of uninstall.sh. Windows
# PowerShell 5.1 (the version supplied with Windows) is enough; the event store is read with
# Windows' own SQLite (winsqlite3.dll).
[CmdletBinding()]
param(
  [switch]$KeepData,
  [switch]$DryRun,
  [string]$DataDir,
  [string]$AppId,
  [switch]$Dev,
  [string]$App,
  [Alias('h')][switch]$Help
)

function Show-Usage {
  @'
Usage: uninstall.ps1 [-KeepData] [-DryRun] [-DataDir PATH] [-AppId ID]
                     [-Dev] [-App PATH] [-Help]

The data directory must already contain brigadier.db and run\. -Dev requires
one explicit ai.brigadier.<name> identifier and -DataDir; it never sweeps
other installations. -App is the folder the app is installed in. Unmerged
branches and dirty worktrees are kept.
'@
}

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

$script:KEEP = @(); $script:UNKNOWN = @(); $script:REMOVED = @(); $script:TRASHED = @()
function Say([string]$text) { [Console]::Out.WriteLine($text) }
function Stop-Uninstall([string]$text) { [Console]::Error.WriteLine("uninstall: $text"); exit 2 }
function Kept([string]$text) { $script:KEEP += $text; Say "KEEP: $text" }
function Uncertain([string]$text) { $script:UNKNOWN += $text; Say "UNCONFIRMED: $text" }
function Removed([string]$text) { $script:REMOVED += $text; Say "REMOVED: $text" }
function InTrash([string]$text) { $script:TRASHED += $text; Say "RECYCLE BIN: $text" }
# git, or in a dry run what it would run. Answers whether it succeeded.
function Invoke-Git([string[]]$arguments) {
  if ($DryRun) { Say ("DRY RUN: git " + ($arguments -join ' ')); return $true }
  & git @arguments
  return $LASTEXITCODE -eq 0
}
# git's output, or nothing when it fails.
function Read-Git([string[]]$arguments) {
  $output = & git @arguments 2>$null
  if ($LASTEXITCODE -ne 0) { return $null }
  return ($output -join "`n")
}
function Same([string]$a, [string]$b) { [string]::Equals($a, $b, [StringComparison]::OrdinalIgnoreCase) }
function Under([string]$path, [string]$folder) {
  $path.StartsWith($folder.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
}

if ($Help) { Show-Usage; exit 0 }

$ID = if ($PSBoundParameters.ContainsKey('AppId')) { $AppId } else { 'ai.brigadier.app' }
$ID_EXPLICIT = $PSBoundParameters.ContainsKey('AppId')
$DATA_EXPLICIT = $PSBoundParameters.ContainsKey('DataDir')
$APP_EXPLICIT = $PSBoundParameters.ContainsKey('App')
$DATA = if ($DATA_EXPLICIT) { $DataDir } elseif ($env:BRIGADIER_DATA_DIR) { $env:BRIGADIER_DATA_DIR } else {
  Join-Path $env:LOCALAPPDATA 'Brigadier\Data'
}
$APP = $App

if ($ID -ceq 'ai.brigadier.app') {
  if ($Dev -or -not $ID_EXPLICIT) { Stop-Uninstall 'the default identifier requires explicit -AppId ai.brigadier.app without -Dev' }
} elseif ($ID -clike 'ai.brigadier.*') {
  if ($ID -cnotmatch '^ai\.brigadier\.[A-Za-z0-9_-]+$') { Stop-Uninstall 'invalid app identifier' }
} else { Stop-Uninstall 'identifier must be ai.brigadier.<name>' }
if ($Dev -and -not ($ID_EXPLICIT -and $DATA_EXPLICIT)) { Stop-Uninstall '-Dev requires -AppId and -DataDir' }
# The installed app's folder, from its uninstall entry (or the installer's default).
$UNINSTALL_KEY = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Brigadier'
if (-not $APP_EXPLICIT -and -not $Dev) {
  $entry = Get-ItemProperty -Path $UNINSTALL_KEY -ErrorAction SilentlyContinue
  $APP = if ($entry -and $entry.PSObject.Properties['InstallLocation']) { $entry.InstallLocation.Trim('"') } else {
    Join-Path $env:LOCALAPPDATA 'Brigadier'
  }
}
if ($DATA -notmatch '^[A-Za-z]:\\') { Stop-Uninstall 'data directory must be an absolute path' }

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class BrigadierUninstall {
  [DllImport("winsqlite3.dll")] static extern int sqlite3_open_v2(byte[] name, out IntPtr db, int flags, IntPtr vfs);
  [DllImport("winsqlite3.dll")] static extern int sqlite3_close(IntPtr db);
  [DllImport("winsqlite3.dll")] static extern int sqlite3_prepare_v2(IntPtr db, IntPtr sql, int bytes, out IntPtr statement, out IntPtr tail);
  [DllImport("winsqlite3.dll")] static extern int sqlite3_step(IntPtr statement);
  [DllImport("winsqlite3.dll")] static extern int sqlite3_finalize(IntPtr statement);
  [DllImport("winsqlite3.dll")] static extern int sqlite3_column_count(IntPtr statement);
  [DllImport("winsqlite3.dll")] static extern IntPtr sqlite3_column_text(IntPtr statement, int column);
  [DllImport("winsqlite3.dll")] static extern int sqlite3_column_bytes(IntPtr statement, int column);
  [DllImport("winsqlite3.dll")] static extern IntPtr sqlite3_errmsg(IntPtr db);

  /// Every row of every statement in `sql`, read-only; NULL reads as "".
  public static List<string[]> Query(string path, string sql) {
    IntPtr db;
    var rows = new List<string[]>();
    // SQLITE_OPEN_READONLY
    if (sqlite3_open_v2(Encoding.UTF8.GetBytes(path + "\0"), out db, 1, IntPtr.Zero) != 0) {
      sqlite3_close(db);
      throw new Exception("cannot open " + path);
    }
    byte[] text = Encoding.UTF8.GetBytes(sql + "\0");
    IntPtr buffer = Marshal.AllocHGlobal(text.Length);
    try {
      Marshal.Copy(text, 0, buffer, text.Length);
      IntPtr next = buffer;
      while (true) {
        IntPtr statement, tail;
        if (sqlite3_prepare_v2(db, next, -1, out statement, out tail) != 0)
          throw new Exception(Marshal.PtrToStringAnsi(sqlite3_errmsg(db)));
        if (statement == IntPtr.Zero) break;
        try {
          int step;
          while ((step = sqlite3_step(statement)) == 100) {
            int count = sqlite3_column_count(statement);
            var row = new string[count];
            for (int i = 0; i < count; i++) {
              IntPtr value = sqlite3_column_text(statement, i);
              int length = sqlite3_column_bytes(statement, i);
              var bytes = new byte[length];
              if (value != IntPtr.Zero) Marshal.Copy(value, bytes, 0, length);
              row[i] = Encoding.UTF8.GetString(bytes);
            }
            rows.Add(row);
          }
          if (step != 101) throw new Exception(Marshal.PtrToStringAnsi(sqlite3_errmsg(db)));
        } finally { sqlite3_finalize(statement); }
        next = tail;
      }
    } finally {
      Marshal.FreeHGlobal(buffer);
      sqlite3_close(db);
    }
    return rows;
  }

  [StructLayout(LayoutKind.Sequential)]
  struct FileInfo {
    public uint Attributes; public long Created, Accessed, Written;
    public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
  }
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
  [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetFileInformationByHandle(IntPtr file, out FileInfo info);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);

  /// The volume and file index of `path` itself (a link is not followed), or null.
  public static string FileId(string path) {
    // Any sharing; FILE_FLAG_BACKUP_SEMANTICS (folders) | FILE_FLAG_OPEN_REPARSE_POINT.
    IntPtr file = CreateFileW(path, 0, 7, IntPtr.Zero, 3, 0x02000000 | 0x00200000, IntPtr.Zero);
    if (file == new IntPtr(-1)) return null;
    try {
      FileInfo info;
      if (!GetFileInformationByHandle(file, out info)) return null;
      return info.Volume + ":" + info.IndexHigh + ":" + info.IndexLow;
    } finally { CloseHandle(file); }
  }

  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  struct FileOperation {
    public IntPtr Window; public uint Func;
    [MarshalAs(UnmanagedType.LPWStr)] public string From;
    [MarshalAs(UnmanagedType.LPWStr)] public string To;
    public ushort Flags; public bool Aborted; public IntPtr Mappings;
    [MarshalAs(UnmanagedType.LPWStr)] public string Title;
  }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode)] static extern int SHFileOperationW(ref FileOperation operation);

  /// Moves `path` to the Recycle Bin without any dialog; answers the error, or null.
  public static string ToRecycleBin(string path) {
    var operation = new FileOperation();
    // FO_DELETE; FOF_SILENT | FOF_NOCONFIRMATION | FOF_ALLOWUNDO | FOF_NOERRORUI
    operation.Func = 3;
    operation.From = path + "\0\0";
    operation.Flags = 0x0004 | 0x0010 | 0x0040 | 0x0400;
    int result = SHFileOperationW(ref operation);
    if (result != 0) return "error " + result;
    if (operation.Aborted) return "aborted";
    return null;
  }
}
'@

function Test-Link([string]$path) {
  try { return ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0 } catch { return $false }
}
# Refuse links and junctions in every component, and dot-dot.
function Test-NoLinks([string]$path) {
  if ($path -notmatch '^[A-Za-z]:\\') { return $false }
  $prefix = $path.Substring(0, 2)
  foreach ($part in $path.Substring(3).Split('\')) {
    if ($part -eq '' -or $part -eq '.') { continue }
    if ($part -eq '..') { return $false }
    $prefix = "$prefix\$part"
    if (Test-Link $prefix) { return $false }
  }
  return $true
}
function Get-FileId([string]$path) { [BrigadierUninstall]::FileId($path) }

if (-not (Test-NoLinks $DATA)) { Stop-Uninstall 'data directory contains a link or unsafe component' }
if (-not (Test-Path -LiteralPath $DATA -PathType Container)) { Stop-Uninstall 'data directory does not exist' }
$DATA = [IO.Path]::GetFullPath($DATA).TrimEnd('\')
if ($DATA.Length -le 3 -or (Same $DATA $env:USERPROFILE)) { Stop-Uninstall 'refusing a drive root or the home folder as data directory' }
if (-not (Test-Path -LiteralPath "$DATA\brigadier.db" -PathType Leaf) -or -not (Test-Path -LiteralPath "$DATA\run" -PathType Container)) {
  Stop-Uninstall 'data directory lacks brigadier.db or run\'
}
if ((Test-Link "$DATA\brigadier.db") -or (Test-Link "$DATA\run")) { Stop-Uninstall 'data directory markers must not be links' }
if (-not (Test-Path -LiteralPath "$env:SystemRoot\System32\winsqlite3.dll")) { Stop-Uninstall 'winsqlite3.dll (Windows 10 or later) is required' }
if ($DATA -match '[\r\n\x1f]') { Stop-Uninstall 'data directory contains unsupported control characters' }
$DATA_ID = Get-FileId $DATA
if (-not $DATA_ID) { Stop-Uninstall 'cannot identify the data directory' }

# The app's uninstall entry: the one whose folder is APP (the installed app's is named Brigadier).
function Get-AppEntry([string]$folder) {
  foreach ($root in 'HKCU:', 'HKLM:') {
    foreach ($key in Get-ChildItem "$root\Software\Microsoft\Windows\CurrentVersion\Uninstall" -ErrorAction SilentlyContinue) {
      $entry = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction SilentlyContinue
      if (-not $entry -or -not $entry.PSObject.Properties['InstallLocation'] -or -not $entry.PSObject.Properties['UninstallString']) { continue }
      if (-not (Same $entry.InstallLocation.Trim('"').TrimEnd('\') $folder)) { continue }
      if ($ID -ceq 'ai.brigadier.app' -and $key.PSChildName -ne 'Brigadier') { continue }
      if (-not (Same $entry.UninstallString.Trim('"') "$folder\uninstall.exe")) { continue }
      return $key.PSPath
    }
  }
  return $null
}
$APP_ENTRY = $null; $APP_ID = $null
if ($APP -and (Test-Path -LiteralPath $APP)) {
  if (-not (Test-NoLinks $APP)) { Stop-Uninstall 'app folder path contains a link or unsafe component' }
  $APP = [IO.Path]::GetFullPath($APP).TrimEnd('\')
  if (-not (Test-Path -LiteralPath "$APP\brigadier.exe" -PathType Leaf)) { Stop-Uninstall 'app folder holds no brigadier.exe' }
  $APP_ENTRY = Get-AppEntry $APP
  if (-not $APP_ENTRY) { Stop-Uninstall 'app folder is not the installation of --app-id (no matching uninstall entry)' }
  $APP_ID = Get-FileId $APP
  if (-not $APP_ID) { Stop-Uninstall 'cannot identify the app folder' }
}

$DB = "$DATA\brigadier.db"
function Read-Store([string]$sql) { , [BrigadierUninstall]::Query($DB, $sql) }
try { Read-Store 'SELECT count(*) FROM events' | Out-Null } catch { Stop-Uninstall 'cannot read the event store' }
Say "Uninstalling $ID from $DATA"

function Test-External([string]$path) {
  ($path -match '^[A-Za-z]:\\') -and ($path -notmatch '[\r\n\x1f]') -and (Test-NoLinks $path)
}

$script:DATA_MOVE_ALLOWED = $false
# Move-ToRecycleBin PATH [ID]: moves PATH to the Recycle Bin if it is still the file ID names (the
# one checked earlier), or, without ID, the one seen when this starts.
function Move-ToRecycleBin([string]$path, [string]$expected = '') {
  if (-not (Test-Path -LiteralPath $path) -and -not (Test-Link $path)) { return $true }
  if (-not $script:DATA_MOVE_ALLOWED -and ((Same $DATA $path) -or (Under $DATA $path))) {
    Uncertain "path contains the data directory and must be resolved separately: $path"; return $false
  }
  if (-not (Test-External $path) -or (Test-Link $path)) { Uncertain "unsafe path left in place: $path"; return $false }
  $id = Get-FileId $path
  if (-not $id) { Uncertain "cannot identify $path"; return $false }
  if ($expected -and $id -ne $expected) { Uncertain "changed since it was checked, left in place: $path"; return $false }
  if ($DryRun) { $script:TRASHED += $path; Say "DRY RUN: move to the Recycle Bin $path"; return $true }
  if ((Test-Link $path) -or (Get-FileId $path) -ne $id -or -not (Test-NoLinks $path)) {
    Uncertain "changed since it was checked, left in place: $path"; return $false
  }
  $why = [BrigadierUninstall]::ToRecycleBin($path)
  if (-not $why -and -not (Test-Path -LiteralPath $path)) { InTrash $path; return $true }
  Uncertain "could not move to the Recycle Bin: $path"; return $false
}

function Get-Daemons {
  $pattern = '(^|\s)--data-dir\s+("?)' + [regex]::Escape($DATA) + '\\?\2(\s|$)'
  Get-CimInstance Win32_Process -Filter "Name = 'brigadierd.exe'" |
    Where-Object { $_.CommandLine -and $_.CommandLine -imatch $pattern } |
    ForEach-Object { [int]$_.ProcessId }
}
function Wait-Gone([int]$processId) {
  for ($i = 0; $i -lt 100; $i++) {
    if (-not (Get-Process -Id $processId -ErrorAction SilentlyContinue)) { return $true }
    Start-Sleep -Milliseconds 100
  }
  return $false
}

# The app first: while it runs it starts its daemon again. An app is identified by the program
# in its installed folder, never by its process name alone.
$APP_RUNNING = $false
if ($APP_ENTRY) {
  $apps = Get-CimInstance Win32_Process -Filter "Name = 'brigadier.exe'" |
    Where-Object { $_.ExecutablePath -and (Same $_.ExecutablePath "$APP\brigadier.exe") }
  foreach ($process in $apps) {
    $processId = [int]$process.ProcessId
    if ($DryRun) { Say "DRY RUN: quit app PID $processId ($APP)"; continue }
    try { Stop-Process -Id $processId -ErrorAction Stop } catch { Uncertain "could not quit app PID $processId" }
    if (-not (Wait-Gone $processId)) { $APP_RUNNING = $true; Uncertain "app PID $processId is still running" }
  }
}

$DAEMON = @(Get-Daemons)
if ($DryRun) {
  if ($DAEMON.Count) { Say "DRY RUN: request orderly daemon shutdown for $DATA (PID $($DAEMON -join ' '))" }
} else {
  $daemonBin = $null
  if ($APP -and (Test-Path -LiteralPath "$APP\brigadierd.exe" -PathType Leaf)) { $daemonBin = "$APP\brigadierd.exe" }
  elseif (Get-Command brigadierd -ErrorAction SilentlyContinue) { $daemonBin = (Get-Command brigadierd).Source }
  $quitCode = 3
  if ($daemonBin) { & $daemonBin quit --data-dir $DATA; $quitCode = $LASTEXITCODE }
  $DAEMON = @(Get-Daemons)
  if ($quitCode -ne 0 -and $DAEMON.Count) {
    foreach ($processId in $DAEMON) {
      try { Stop-Process -Id $processId -ErrorAction Stop } catch { Uncertain "cannot stop daemon PID $processId" }
    }
  }
  foreach ($processId in $DAEMON) {
    if (-not (Wait-Gone $processId)) { Uncertain "daemon PID $processId is still running" }
  }
  if (@(Get-Daemons).Count) { Uncertain "daemon still serves $DATA"; Say 'Daemon is still running; no files were removed.'; exit 1 }
  if ($quitCode -eq 1 -and -not $DAEMON.Count) { Uncertain "daemon shutdown for $DATA could not be confirmed"; exit 1 }
}

# Only literal paths from cleanup.recorded qualify. Historical removed/completed entries are
# omitted; a later re-record of the same artifact qualifies again.
try {
  $ledger = Read-Store @'
SELECT json_extract(r.payload,'$.artifact.type'),
  COALESCE(json_extract(r.payload,'$.artifact.path'),json_extract(r.payload,'$.artifact.sessionId'),json_extract(r.payload,'$.artifact.threadId'),''),
  COALESCE(json_extract(r.payload,'$.artifact.repo'),'')
  FROM events r WHERE r.stream='cleanup' AND r.kind='cleanup.recorded'
  AND NOT EXISTS (SELECT 1 FROM events e WHERE e.stream='cleanup' AND e.seq>r.seq
    AND json_extract(e.payload,'$.owner')=json_extract(r.payload,'$.owner')
    AND (e.kind='cleanup.completed' OR
      (e.kind='cleanup.removed' AND EXISTS
        (SELECT 1 FROM json_each(e.payload,'$.artifacts') a
         WHERE a.value=json_extract(r.payload,'$.artifact')))))
  GROUP BY r.payload ORDER BY r.seq
'@
} catch { Stop-Uninstall 'could not query cleanup ledger' }

$WORKTREES = @(); $REPOS = @()
foreach ($row in $ledger) {
  $type, $value, $repo = $row
  if ($type -ne 'worktree') { continue }
  if ((Under $value "$DATA\worktrees") -and (Test-External $value) -and (Test-External $repo)) {
    $WORKTREES += , @($repo, $value)
    if (-not ($REPOS | Where-Object { Same $_ $repo })) { $REPOS += $repo }
  } else { Uncertain "unsafe ledger worktree: $value" }
}

$BRANCH_REPOS = @()
foreach ($item in $WORKTREES) {
  $repo, $path = $item
  if (Test-Path -LiteralPath $path -PathType Container) {
    $branch = Read-Git @('-C', $path, 'symbolic-ref', '--quiet', '--short', 'HEAD')
    if ($branch) { $BRANCH_REPOS += , @($branch, $repo) }
  }
}

$PLANNED = @()
foreach ($item in $WORKTREES) {
  $repo, $path = $item
  if (-not (Test-Path -LiteralPath $path)) { continue }
  if (-not (Test-Path -LiteralPath "$repo\.git")) { Kept "worktree $path; repository unavailable: $repo"; continue }
  if ($DryRun) {
    if (Read-Git @('-C', $path, 'status', '--porcelain')) {
      Kept "worktree $path has changes; resolve it, then run: git -C '$repo' worktree remove '$path'"
    } else {
      Say "DRY RUN: git -C $repo worktree remove $path"
      $PLANNED += $path
    }
    continue
  }
  & git -C $repo worktree remove $path
  if ($LASTEXITCODE -eq 0) { Removed "worktree $path" }
  else { Kept "worktree $path (possibly dirty). Resolve it, then run: git -C '$repo' worktree remove '$path'" }
}
foreach ($repo in $REPOS) {
  if (-not (Invoke-Git @('-C', $repo, 'worktree', 'prune'))) { Uncertain "could not prune git worktrees in $repo" }
}

# Which branches are Brigadier's, in which repository, and what each one's work lands on (its
# target). A branch qualifies only as recorded, in its recorded repository: first the branches
# kept when their conversation went (bound to the tip they had then), then session branches
# from their session's setup, then task branches with their session's repository.
try {
  $branches = Read-Store @'
SELECT json_extract(e.payload,'$.repo'), json_extract(b.value,'$.name'),
    json_extract(b.value,'$.target'), json_extract(b.value,'$.tip'), ''
  FROM events e, json_each(e.payload,'$.branches') b WHERE e.kind='branches.kept';
SELECT json_extract(payload,'$.setup.repo'), json_extract(payload,'$.setup.environment.branch'),
    json_extract(payload,'$.setup.environment.base'), '', ''
  FROM events WHERE kind='conversation.setUp'
    AND json_extract(payload,'$.setup.environment.type')='newWorktree';
SELECT s.repo, t.branch, t.target, '', t.worktree FROM
  (SELECT json_extract(payload,'$.task.conversationId') AS conversation,
      json_extract(payload,'$.task.workspace.branch') AS branch,
      json_extract(payload,'$.task.workspace.target') AS target,
      json_extract(payload,'$.task.workspace.worktree') AS worktree, MAX(seq)
    FROM events WHERE kind='task.updated'
    GROUP BY json_extract(payload,'$.task.id')) t
  LEFT JOIN
  (SELECT json_extract(payload,'$.id') AS id, json_extract(payload,'$.setup.repo') AS repo, MAX(seq)
    FROM events WHERE kind='conversation.setUp' GROUP BY json_extract(payload,'$.id')) s
  ON s.id=t.conversation
  WHERE t.branch IS NOT NULL
'@
} catch { Stop-Uninstall 'could not query branch records' }

$HANDLED = @()
foreach ($row in $branches) {
  $repo, $branch, $target, $tip, $worktree = $row[0..4]
  if (-not $branch.StartsWith('brigadier/')) { continue }
  & git check-ref-format "refs/heads/$branch" 2>$null
  if ($LASTEXITCODE -ne 0) { Uncertain "unsafe branch name: $branch"; continue }
  # A task whose session record is gone: its worktree's ledger record names the repository.
  if (-not $repo -and $worktree) {
    foreach ($item in $WORKTREES) { if (Same $item[1] $worktree) { $repo = $item[0] } }
  }
  if (-not $repo -or -not (Test-External $repo) -or -not (Test-Path -LiteralPath $repo -PathType Container)) {
    Uncertain "branch $branch has no confirmed repository; inspect it manually"; continue
  }
  $key = "$branch`n$($repo.ToLowerInvariant())"
  if ($HANDLED -contains $key) { continue }
  $HANDLED += $key
  $current = Read-Git @('-C', $repo, 'rev-parse', '--verify', '--quiet', "refs/heads/$branch^{commit}")
  if (-not $current) { continue }
  $manual = "git -C '$repo' branch -D '$branch'"
  $targetTip = $null
  $targetValid = $target -and -not $target.StartsWith('-')
  if ($targetValid) { & git check-ref-format "refs/heads/$target" 2>$null; $targetValid = $LASTEXITCODE -eq 0 }
  if ($targetValid) { $targetTip = Read-Git @('-C', $repo, 'rev-parse', '--verify', '--quiet', "refs/heads/$target^{commit}") }
  if ($tip -and $current -ne $tip) {
    Kept "branch $branch changed since Brigadier recorded it; delete manually if wanted: $manual"
  } elseif (-not $targetValid) {
    Uncertain "branch $branch has no recorded target; kept. Delete manually if wanted: $manual"
  } elseif (-not $targetTip) {
    Uncertain "branch ${branch}: its target $target is gone; kept. Delete manually if wanted: $manual"
  } else {
    & git -C $repo merge-base --is-ancestor $current $targetTip 2>$null
    if ($LASTEXITCODE -ne 0) { Kept "unmerged branch $branch; delete manually if wanted: $manual" }
    elseif ($DryRun) { Say "DRY RUN: git -C $repo branch -d $branch" }
    else {
      & git -C $repo branch -d $branch
      if ($LASTEXITCODE -eq 0) { Removed "merged branch $branch in $repo" }
      else { Uncertain "git refused safe deletion of branch $branch; inspect: git -C '$repo' branch -d '$branch'" }
    }
  }
}
foreach ($item in $BRANCH_REPOS) {
  $branch, $repo = $item
  if (-not $branch.StartsWith('brigadier/')) { continue }
  if ($HANDLED -contains "$branch`n$($repo.ToLowerInvariant())") { continue }
  & git -C $repo show-ref --verify --quiet "refs/heads/$branch"
  if ($LASTEXITCODE -eq 0) {
    Uncertain "recorded branch $branch has no base; kept. Delete manually if wanted: git -C '$repo' branch -D '$branch'"
  }
}

$CLAUDE_ROOT = if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR.TrimEnd('\') } else { "$env:USERPROFILE\.claude" }
$CODEX_ROOT = if ($env:CODEX_HOME) { $env:CODEX_HOME.TrimEnd('\') } else { "$env:USERPROFILE\.codex" }
function Get-Matching([string]$folder, [string]$filter) {
  if (-not (Test-Path -LiteralPath $folder -PathType Container)) { return @() }
  @(Get-ChildItem -LiteralPath $folder -Filter $filter -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
}
foreach ($row in $ledger) {
  $type, $value, $repo = $row
  switch ($type) {
    'claudeSession' {
      if ($value -cmatch '^[A-Za-z0-9_-]+$') {
        if (Test-Path -LiteralPath "$CLAUDE_ROOT\projects" -PathType Container) {
          foreach ($dir in Get-ChildItem -LiteralPath "$CLAUDE_ROOT\projects" -Directory -Force) {
            if (Test-Link $dir.FullName) { continue }
            Move-ToRecycleBin "$($dir.FullName)\$value.jsonl" | Out-Null
            Move-ToRecycleBin "$($dir.FullName)\$value" | Out-Null
          }
        }
        foreach ($path in "$CLAUDE_ROOT\tasks\$value", "$CLAUDE_ROOT\session-env\$value", "$CLAUDE_ROOT\file-history\$value", "$CLAUDE_ROOT\debug\$value.txt") {
          Move-ToRecycleBin $path | Out-Null
        }
        foreach ($file in Get-Matching "$CLAUDE_ROOT\todos" "$value-*.json") { Move-ToRecycleBin $file | Out-Null }
      } else { Uncertain "invalid Claude session id: $value" }
    }
    'claudeProjectDir' {
      if ((Under $value "$CLAUDE_ROOT\projects") -and (Test-External $value)) { Move-ToRecycleBin $value | Out-Null }
      else { Uncertain "unsafe Claude project path: $value" }
    }
    'claudeStagingDir' {
      if ((Test-External $value) -and (Test-Path -LiteralPath $value -PathType Container) -and -not (Get-ChildItem -LiteralPath $value -Force)) {
        if ($DryRun) { Say "DRY RUN: remove empty staging directory $value" }
        else {
          try { Remove-Item -LiteralPath $value -ErrorAction Stop; Removed $value } catch { Uncertain "could not remove empty staging directory $value" }
        }
      } elseif (Test-Path -LiteralPath $value) { Uncertain "Claude staging directory is not empty: $value" }
    }
    'codexGeneratedImages' {
      if ((Under $value "$CODEX_ROOT\generated_images") -and (Test-External $value)) { Move-ToRecycleBin $value | Out-Null }
      else { Uncertain "unsafe Codex image path: $value" }
    }
    'codexThread' {
      if ($value -match '^[A-Fa-f0-9-]{36}$') {
        if (Test-Path -LiteralPath "$CODEX_ROOT\sessions" -PathType Container) {
          foreach ($file in Get-ChildItem -Path "$CODEX_ROOT\sessions\*\*\*\rollout-*$value.jsonl" -File -Force -ErrorAction SilentlyContinue) {
            Move-ToRecycleBin $file.FullName | Out-Null
          }
        }
        Uncertain "Codex state DB row for thread $value must be checked in Codex"
      } else { Uncertain "invalid Codex thread id: $value" }
    }
    'codexProjectTrust' { Uncertain "Codex config.toml trust entry for $value must be checked manually" }
    # Brigadier makes Claude session temp folders only on macOS and Linux.
    'claudeTempDir' { Uncertain "unsafe recorded temp path: $value" }
  }
}

# The keep-awake rule and the microphone permission are macOS's; Windows has neither.
foreach ($path in "$env:APPDATA\$ID", "$env:LOCALAPPDATA\$ID") { Move-ToRecycleBin $path | Out-Null }

# A remaining worktree blocks the parent data directory, even with -KeepData.
$remaining = $false
if (Test-Path -LiteralPath "$DATA\worktrees" -PathType Container) {
  foreach ($session in Get-ChildItem -LiteralPath "$DATA\worktrees" -Directory -Force) {
    foreach ($dir in Get-ChildItem -LiteralPath $session.FullName -Directory -Force) {
      $path = $dir.FullName
      if ($DryRun -and ($PLANNED | Where-Object { Same $_ $path })) { continue }
      $remaining = $true
      $repo = $null
      foreach ($item in $WORKTREES) { if (Same $item[1] $path) { $repo = $item[0] } }
      if ($repo) { Kept "remaining worktree $path; resolve and run: git -C '$repo' worktree remove '$path'" }
      else { Kept "unrecorded worktree $path; inspect before removing it" }
    }
  }
}
foreach ($file in "$DATA\run\brigadierd.lock", "$DATA\run\ipc.token") {
  if (-not (Test-Path -LiteralPath $file)) { continue }
  if ($DryRun) { Say "DRY RUN: remove runtime file $file"; continue }
  try {
    if (Test-Link $file) { throw 'link' }
    Remove-Item -LiteralPath $file -Force -ErrorAction Stop; Removed $file
  } catch { Uncertain "runtime file left: $file" }
}
if ($KeepData) { Kept "data directory $DATA (-KeepData)" }
elseif ($remaining) { Kept "data directory $DATA (worktrees remain)" }
else {
  $script:DATA_MOVE_ALLOWED = $true
  Move-ToRecycleBin $DATA $DATA_ID | Out-Null
}

# The app goes with its own uninstaller, which removes only what its installer put there.
if ($APP_ENTRY -and (Test-Path -LiteralPath $APP)) {
  if ($APP_RUNNING) { Kept "app $APP (process still running)" }
  elseif ((Get-FileId $APP) -ne $APP_ID) { Uncertain "changed since it was checked, left in place: $APP" }
  elseif ($DryRun) { Say "DRY RUN: run the app's uninstaller $APP\uninstall.exe /S" }
  else {
    Start-Process -FilePath "$APP\uninstall.exe" -ArgumentList '/S' -Wait
    # The uninstaller runs on from a copy of itself in the temp folder.
    for ($i = 0; $i -lt 600 -and ((Test-Path -LiteralPath "$APP\uninstall.exe") -or (Test-Path -LiteralPath $APP_ENTRY)); $i++) {
      Start-Sleep -Milliseconds 100
    }
    if ((Test-Path -LiteralPath "$APP\brigadier.exe") -or (Test-Path -LiteralPath $APP_ENTRY)) { Uncertain "the app's uninstaller left $APP in place" }
    else { Removed "app $APP (its uninstaller)" }
  }
}

Say ''; Say 'Summary:'
Say "  Removed directly: $($script:REMOVED.Count)"
if ($DryRun) { Say "  Would move to the Recycle Bin: $($script:TRASHED.Count)" }
else { Say "  Moved to the Recycle Bin: $($script:TRASHED.Count) (empty it to reclaim space)" }
Say "  Kept: $($script:KEEP.Count)"
Say "  Unconfirmed: $($script:UNKNOWN.Count)"
if ($remaining -or $script:UNKNOWN.Count) { exit 1 }

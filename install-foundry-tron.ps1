<#
.SYNOPSIS
  Keyless installer for the *-tron CLIs (forge-tron, cast-tron, anvil-tron,
  chisel-tron) on Windows.

.DESCRIPTION
  Downloads the win32-amd64 archive from the rolling `foundry-tron-latest`
  prerelease of elsvv/foundry-tron, verifies its SHA-256 checksum when the
  release publishes one, unpacks the four .exe binaries into
  %USERPROFILE%\.foundry-tron\bin, and prints a PATH hint. Re-runs are
  idempotent.

.PARAMETER Dir
  Install into this directory instead of %USERPROFILE%\.foundry-tron.

.PARAMETER Tag
  Pull from a different release tag (default: foundry-tron-latest).

.PARAMETER ModifyPath
  Prepend the bin directory to the current user's PATH (persisted).

.PARAMETER RequireChecksum
  Fail if the release has no checksum to verify against.

.EXAMPLE
  irm https://raw.githubusercontent.com/elsvv/foundry-tron/master/install-foundry-tron.ps1 | iex

.EXAMPLE
  .\install-foundry-tron.ps1 -ModifyPath
#>

[CmdletBinding()]
param(
  [string]$Dir = (Join-Path $env:USERPROFILE ".foundry-tron"),
  [string]$Tag = "foundry-tron-latest",
  [switch]$ModifyPath,
  [switch]$RequireChecksum
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Repo = "elsvv/foundry-tron"
$Asset = "foundry-tron_win32_amd64.zip"
$Binaries = @("forge-tron.exe", "cast-tron.exe", "anvil-tron.exe", "chisel-tron.exe")
$BinDir = Join-Path $Dir "bin"

function Say($msg) { Write-Host "install-foundry-tron: $msg" }
function Warn($msg) { Write-Warning "install-foundry-tron: $msg" }

# Download a release asset over plain HTTPS, falling back to `gh release download`
# when the asset host is unreachable (corp or regional blocks).
function Get-Asset($name, $dest) {
  $url = "https://github.com/$Repo/releases/download/$Tag/$name"
  try {
    Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
    return $true
  } catch {
    Say "direct download failed; trying 'gh release download'"
    if (Get-Command gh -ErrorAction SilentlyContinue) {
      $dir = Split-Path -Parent $dest
      & gh release download $Tag --repo $Repo --pattern $name --dir $dir --clobber 2>$null
      if ($LASTEXITCODE -eq 0) {
        $downloaded = Join-Path $dir $name
        if ($downloaded -ne $dest) { Move-Item -Force $downloaded $dest }
        return $true
      }
    }
    return $false
  }
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("foundry-tron-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
  Say "installing the *-tron CLIs from $Repo@$Tag"
  Say "target: $BinDir"

  $archive = Join-Path $tmp $Asset
  Say "downloading $Asset"
  if (-not (Get-Asset $Asset $archive)) {
    throw "could not download $Asset (checked direct download and gh)"
  }

  # Verify the SHA-256 sidecar when the release publishes one.
  $sumFile = Join-Path $tmp "$Asset.sha256"
  if (Get-Asset "$Asset.sha256" $sumFile) {
    $expected = ((Get-Content $sumFile -Raw).Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLower()
    if ($expected -ne $actual) {
      throw "checksum mismatch for $Asset (expected $expected, got $actual)"
    }
    Say "checksum verified ($actual)"
  } elseif ($RequireChecksum) {
    throw "no checksum published for $Asset and -RequireChecksum was set"
  } else {
    Warn "no checksum published for $Asset; skipping integrity verification"
  }

  $stage = Join-Path $tmp "stage"
  Expand-Archive -Path $archive -DestinationPath $stage -Force
  foreach ($b in $Binaries) {
    if (-not (Test-Path (Join-Path $stage $b))) { throw "archive is missing $b" }
  }

  New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
  foreach ($b in $Binaries) {
    Move-Item -Force (Join-Path $stage $b) (Join-Path $BinDir $b)
  }
  Say "installed: $($Binaries -join ', ')"

  $forge = Join-Path $BinDir "forge-tron.exe"
  try {
    $ver = (& $forge --version 2>$null | Select-Object -First 1)
    if ($ver) { Say "forge-tron reports: $ver" }
  } catch {
    Warn "forge-tron --version did not run cleanly; check your platform"
  }

  if ($ModifyPath) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    if (($userPath -split ';') -notcontains $BinDir) {
      $newPath = if ($userPath) { "$BinDir;$userPath" } else { $BinDir }
      [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
      Say "added $BinDir to your user PATH (open a new terminal to pick it up)"
    } else {
      Say "$BinDir already on your user PATH"
    }
  } else {
    if (($env:Path -split ';') -contains $BinDir) {
      Say "$BinDir is already on your PATH; you're ready to go"
    } else {
      Say "add $BinDir to your PATH, or re-run with -ModifyPath, e.g.:"
      Write-Host ""
      Write-Host "  `$env:Path = `"$BinDir;`$env:Path`""
      Write-Host ""
    }
  }

  Say "done. try: forge-tron --version"
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

<#
.SYNOPSIS
  tvpn installer for Windows (PowerShell 5.1+ and PowerShell 7).

.DESCRIPTION
  PowerShell:  irm https://raw.githubusercontent.com/OWNER/tvpn/main/install.ps1 | iex
  cmd.exe:     powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/OWNER/tvpn/main/install.ps1 | iex"

  Installs tvpn.exe and wintun.dll to %LOCALAPPDATA%\Programs\tvpn and adds it
  to your user PATH. Downloads are verified against the release SHA256SUMS.
  Overrides: $env:TVPN_VERSION, $env:TVPN_INSTALL_DIR, $env:TVPN_REPO,
  $env:TVPN_DOWNLOAD_BASE (a mirror URL containing the release assets),
  $env:TVPN_NO_MODIFY_PATH=1 (don't touch the user PATH).
#>
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = if ($env:TVPN_REPO) { $env:TVPN_REPO } else { 'OWNER/tvpn' }
$Version = if ($env:TVPN_VERSION) { $env:TVPN_VERSION.TrimStart('v') } else { 'latest' }
$InstallDir = if ($env:TVPN_INSTALL_DIR) { $env:TVPN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\tvpn' }

function Say($msg) { Write-Host "tvpn-install: $msg" }

# Windows PowerShell 5.1 defaults to old TLS versions.
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch {}

$arch = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
switch ($arch) {
    'AMD64' { $target = 'x86_64-pc-windows-msvc' }
    'ARM64' { $target = 'aarch64-pc-windows-msvc' }
    default { throw "tvpn-install: unsupported architecture $arch" }
}

$base = if ($env:TVPN_DOWNLOAD_BASE) { $env:TVPN_DOWNLOAD_BASE.TrimEnd('/') } elseif ($Version -eq 'latest') { "https://github.com/$Repo/releases/latest/download" } else { "https://github.com/$Repo/releases/download/v$Version" }
$asset = "tvpn-$target.zip"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("tvpn-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Say "downloading $asset ($Version)"
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS')

    $line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))$" } | Select-Object -First 1
    if (-not $line) { throw "tvpn-install: $asset is not listed in SHA256SUMS" }
    $expected = ($line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLower()
    if ($expected -ne $actual) { throw "tvpn-install: checksum mismatch for $asset (expected $expected, got $actual)" }
    Say 'checksum verified'

    $extract = Join-Path $tmp 'x'
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $extract -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Get-ChildItem -Path $extract -Recurse -File | Where-Object { $_.Name -in @('tvpn.exe', 'wintun.dll') } | ForEach-Object {
        Copy-Item -Force $_.FullName (Join-Path $InstallDir $_.Name)
    }
    if (-not (Test-Path (Join-Path $InstallDir 'tvpn.exe'))) { throw 'tvpn-install: archive did not contain tvpn.exe' }
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

if (-not $env:TVPN_NO_MODIFY_PATH) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    if (($userPath -split ';') -notcontains $InstallDir) {
        $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Say "added $InstallDir to your user PATH (open a new terminal to use it)"
    }
}
if (($env:Path -split ';') -notcontains $InstallDir) { $env:Path = "$env:Path;$InstallDir" }

$installed = & (Join-Path $InstallDir 'tvpn.exe') --version
Say "installed $installed to $InstallDir"
Say "next: open a terminal as Administrator and run 'tvpn doctor', 'tvpn import <file.conf>', 'tvpn up'"

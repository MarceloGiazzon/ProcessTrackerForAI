# Installs the latest ptrack release on Windows.
#
#   irm https://raw.githubusercontent.com/MarceloGiazzon/ProcessTrackerForAI/main/install.ps1 | iex
#
# Options (environment variables):
#   $env:PTRACK_VERSION = "v0.1.0"      install a specific version instead of the latest
#   $env:PTRACK_INSTALL_DIR = "C:\..."  where to put ptrack.exe (default: %LOCALAPPDATA%\Programs\ptrack)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repo = "MarceloGiazzon/ProcessTrackerForAI"
$dir = if ($env:PTRACK_INSTALL_DIR) { $env:PTRACK_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\ptrack" }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }
$target = "$arch-pc-windows-msvc"

$version = $env:PTRACK_VERSION
if (-not $version) {
    $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
}

$name = "ptrack-$version-$target.zip"
$url = "https://github.com/$repo/releases/download/$version/$name"
$work = Join-Path ([IO.Path]::GetTempPath()) ("ptrack-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $work | Out-Null

try {
    Write-Host "Downloading ptrack $version for $target..."
    $zip = Join-Path $work $name
    Invoke-WebRequest $url -OutFile $zip
    try {
        $expected = ((Invoke-WebRequest "$url.sha256").Content.ToString().Trim() -split '\s+')[0]
        $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) { throw "checksum mismatch; the download may be corrupted" }
    } catch [System.Net.WebException] { }

    Expand-Archive $zip -DestinationPath $work -Force
    New-Item -ItemType Directory -Force $dir | Out-Null
    $exe = Join-Path $dir "ptrack.exe"
    # Windows cannot overwrite a running .exe (a background job's supervisor
    # may be using it), but it can rename it. Running jobs keep the old copy.
    if (Test-Path $exe) {
        Remove-Item "$exe.old" -Force -ErrorAction SilentlyContinue
        if (Test-Path "$exe.old") { $old = "$exe.$([guid]::NewGuid()).old" } else { $old = "$exe.old" }
        Move-Item $exe $old -Force
    }
    Copy-Item (Join-Path $work "ptrack.exe") $exe -Force
} finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $userPath) { $userPath = "" }
if (-not (($userPath -split ';') -contains $dir)) {
    [Environment]::SetEnvironmentVariable("Path", (($userPath.TrimEnd(';'), $dir) -join ';').TrimStart(';'), "User")
    $env:Path = "$env:Path;$dir"
    Write-Host "Added $dir to your PATH (open a new terminal for other windows to see it)."
}

Write-Host "Installed: $(Join-Path $dir 'ptrack.exe')"
& (Join-Path $dir "ptrack.exe") version

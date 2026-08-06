# Installs mstream-player on Windows.
#
#   irm https://raw.githubusercontent.com/IrosTheBeggar/mstream-terminal-player/main/install.ps1 | iex
#
# Fetches the Windows binary from the latest GitHub release, checks its
# sha256 against the release's manifest.json, installs it under
# %LOCALAPPDATA%\Programs\mstream-player, and puts that directory on the
# user PATH. Configuration, all optional, via environment variables,
# because the pipe-to-iex form cannot take parameters:
#
#   MSTREAM_PLAYER_VERSION      a tag like v0.1.0 (default: latest)
#   MSTREAM_PLAYER_INSTALL_DIR  where the binary goes
#   MSTREAM_PLAYER_NO_PATH      set to 1 to leave PATH alone
#
# Windows PowerShell 5.1 is the floor. That is why TLS 1.2 is asked for by
# hand (5.1 predates it being the default), and why this file is pure
# ASCII: without a BOM, 5.1 reads UTF-8 as the ANSI codepage, where the
# last byte of an em dash turns into a curly quote that PowerShell will
# happily treat as a string terminator.
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072

$repo = 'IrosTheBeggar/mstream-terminal-player'
$asset = 'mstream-player-win32-x64.exe'
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw "no prebuilt binary for $env:PROCESSOR_ARCHITECTURE - x64 only for now"
}

$version = if ($env:MSTREAM_PLAYER_VERSION) { $env:MSTREAM_PLAYER_VERSION } else { 'latest' }
$dir = if ($env:MSTREAM_PLAYER_INSTALL_DIR) {
    $env:MSTREAM_PLAYER_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA 'Programs\mstream-player'
}
$base = if ($version -eq 'latest') {
    "https://github.com/$repo/releases/latest/download"
} else {
    "https://github.com/$repo/releases/download/$version"
}

$tmp = Join-Path $env:TEMP "mstream-player-install-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    Write-Host "fetching $asset ($version)..."
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest -UseBasicParsing -Uri "$base/manifest.json" -OutFile (Join-Path $tmp 'manifest.json')

    $manifest = Get-Content (Join-Path $tmp 'manifest.json') -Raw | ConvertFrom-Json
    $expected = ($manifest.assets | Where-Object file -eq $asset).sha256
    if (-not $expected) {
        throw "manifest.json has no entry for $asset - refusing to install"
    }
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLower()
    if ($actual -ne $expected) {
        throw "sha256 mismatch for $asset - download corrupted, not installing (expected $expected, got $actual)"
    }

    New-Item -ItemType Directory -Force $dir | Out-Null
    Move-Item -Force (Join-Path $tmp $asset) (Join-Path $dir 'mstream-player.exe')
    $installed = & (Join-Path $dir 'mstream-player.exe') --version
    Write-Host "installed $installed to $dir"

    if (-not $env:MSTREAM_PLAYER_NO_PATH) {
        # The user PATH in the registry, not this session's: an installer
        # that edits $env:PATH improves exactly one window's life.
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (($userPath -split ';') -notcontains $dir) {
            [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
            Write-Host "added $dir to your user PATH - new terminals will see it"
        }
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

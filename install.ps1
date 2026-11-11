# cswap installer for Windows: https://github.com/sidhanthapoddar99/claude-swap-rs
#
#   irm https://raw.githubusercontent.com/sidhanthapoddar99/claude-swap-rs/main/install.ps1 | iex
#
# Saved to a file, it takes switches:
#   -Dir <path>       install directory (default %LOCALAPPDATA%\Programs\cswap)
#   -NoModifyPath     do not add the directory to your user PATH
#   -Profile          add the shell integration to your PowerShell profile without asking
#   -NoProfile        never touch the profile, just print the line
param(
    [string]$Dir = $(if ($env:CSWAP_INSTALL_DIR) { $env:CSWAP_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\cswap' }),
    [switch]$NoModifyPath,
    [switch]$Profile,
    [switch]$NoProfile
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repo = 'sidhanthapoddar99/claude-swap-rs'

# One build serves both: Windows on ARM runs x64 programs.
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
    throw "There is no prebuilt cswap for $arch. Build it with: cargo install --git https://github.com/$repo"
}
$asset = 'cswap-x86_64-pc-windows-msvc.zip'
$url = "https://github.com/$repo/releases/latest/download/$asset"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("cswap-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading $asset (latest release)..."
    $zip = Join-Path $tmp $asset
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
    Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile "$zip.sha256"

    Write-Host 'Verifying checksum...'
    $expected = ((Get-Content "$zip.sha256" -Raw).Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    if ($expected -ne $actual) {
        throw "The download does not match its checksum (expected $expected, got $actual). Nothing was installed."
    }

    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $exe = Join-Path $Dir 'cswap.exe'
    if (Test-Path $exe) {
        # A running cswap cannot be overwritten, but it can be moved aside.
        $old = Join-Path $Dir 'cswap.old.exe'
        Remove-Item $old -Force -ErrorAction SilentlyContinue
        Move-Item $exe $old -Force
    }
    Move-Item (Join-Path $tmp 'cswap.exe') $exe -Force
    Write-Host "Installed $exe ($(& $exe --version))"
}
finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# --- PATH -------------------------------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = ($userPath -split ';') -contains $Dir
if (-not $onPath) {
    if ($NoModifyPath) {
        Write-Host "NOTE: $Dir is not on your PATH. Add it to run 'cswap' from anywhere."
    }
    else {
        [Environment]::SetEnvironmentVariable('Path', (($userPath.TrimEnd(';') + ';' + $Dir).TrimStart(';')), 'User')
        $env:Path = "$env:Path;$Dir"
        Write-Host "Added $Dir to your user PATH. New terminals will see it."
    }
}

# --- shell integration --------------------------------------------------------
$line = 'Invoke-Expression (& cswap shell-init powershell | Out-String)'
$profilePath = $PROFILE.CurrentUserAllHosts
$has = (Test-Path $profilePath) -and (Select-String -Path $profilePath -SimpleMatch 'cswap shell-init' -Quiet)
if ($has) {
    Write-Host "Shell integration is already in $profilePath."
}
else {
    $add = [bool]$Profile
    if (-not $Profile -and -not $NoProfile -and [Environment]::UserInteractive) {
        Write-Host "Shell integration makes 'claude', 'codex', 'agy' and 'grok' run through cswap."
        $reply = Read-Host "Add it to $profilePath? [Y/n]"
        $add = $reply -notmatch '^[Nn]'
    }
    if ($add) {
        New-Item -ItemType Directory -Force -Path (Split-Path $profilePath) | Out-Null
        Add-Content -Path $profilePath -Value "`n# >>> cswap shell integration >>>`n$line`n# <<< cswap shell integration <<<"
        Write-Host "Added to $profilePath. Open a new terminal to pick it up."
    }
    else {
        Write-Host ''
        Write-Host "Shell integration was not installed. Without it, start agents with"
        Write-Host "'cswap claude', 'cswap codex' and so on. To make the plain commands go"
        Write-Host 'through cswap, add this line to your PowerShell profile:'
        Write-Host "  $line"
    }
}

Write-Host ''
Write-Host 'Next:'
Write-Host '  cswap                   open the dashboard'
Write-Host '  cswap add claude work   add a second Claude account named work'
Write-Host '  cswap -h                every command'

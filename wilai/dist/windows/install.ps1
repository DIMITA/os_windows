# install.ps1 — one-shot installer for the Wilai Windows bridge.
#
# What it does:
#   1. Copies wilai.ps1 into %LOCALAPPDATA%\Programs\wilai\.
#   2. Writes a wilai.cmd stub so `wilai` works in cmd + PowerShell.
#   3. Adds that folder to the user PATH (per-user, no admin needed).
#   4. Creates %APPDATA%\wilai\config.toml with sensible defaults if
#      it does not already exist.
#   5. Prints a short "what next" summary.
#
# Run as:
#   PowerShell -ExecutionPolicy Bypass -File install.ps1
#   PowerShell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
#   PowerShell -ExecutionPolicy Bypass -File install.ps1 -Mode ssh -Host wilai@my-box.lan
#   PowerShell -ExecutionPolicy Bypass -File install.ps1 -Mode wsl -Distro WilOS

[CmdletBinding()]
param(
    [switch]$Uninstall,
    [ValidateSet('wsl','ssh','ask')]
    [string]$Mode = 'ask',
    [string]$Distro,
    [string]$Host
)

$ErrorActionPreference = 'Stop'

$Here      = Split-Path -Parent $MyInvocation.MyCommand.Path
$InstallTo = Join-Path $env:LOCALAPPDATA 'Programs\wilai'
$CfgDir    = Join-Path $env:APPDATA 'wilai'
$CfgFile   = Join-Path $CfgDir 'config.toml'

function Write-Step([string]$msg) { Write-Host ">> $msg" -ForegroundColor Cyan }

function Add-PathIfMissing([string]$path) {
    $cur = [Environment]::GetEnvironmentVariable('Path','User')
    if (-not $cur) { $cur = '' }
    $parts = $cur -split ';' | Where-Object { $_ -ne '' }
    if ($parts -contains $path) { return $false }
    $new = ($parts + $path) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $new, 'User')
    return $true
}

function Remove-PathIfPresent([string]$path) {
    $cur = [Environment]::GetEnvironmentVariable('Path','User')
    if (-not $cur) { return $false }
    $parts = $cur -split ';' | Where-Object { $_ -ne '' -and $_ -ne $path }
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
    return $true
}

# --- uninstall ---------------------------------------------------------

if ($Uninstall) {
    Write-Step "removing $InstallTo"
    if (Test-Path $InstallTo) { Remove-Item -Recurse -Force $InstallTo }
    Remove-PathIfPresent $InstallTo | Out-Null
    Write-Host "wilai uninstalled. $CfgDir left in place (your config, your call)."
    return
}

# --- install -----------------------------------------------------------

Write-Step "install directory: $InstallTo"
New-Item -ItemType Directory -Force -Path $InstallTo | Out-Null

# Prefer the native .exe when it's available next to install.ps1 —
# that's what the release zip ships. Fall back to the .ps1 launcher
# otherwise (useful when hacking in-tree).
$exeSrc = Join-Path $Here 'wilai.exe'
$ps1Src = Join-Path $Here 'wilai.ps1'

if (Test-Path $exeSrc) {
    Write-Step "installing native wilai.exe"
    Copy-Item -Force $exeSrc (Join-Path $InstallTo 'wilai.exe')
    # Keep .ps1 beside it for debugging/fallback, don't require it.
    if (Test-Path $ps1Src) {
        Copy-Item -Force $ps1Src (Join-Path $InstallTo 'wilai.ps1')
    }
} elseif (Test-Path $ps1Src) {
    Write-Step "installing PowerShell launcher (no wilai.exe found)"
    Copy-Item -Force $ps1Src (Join-Path $InstallTo 'wilai.ps1')
    # wilai.cmd stub so `wilai` works in a plain cmd shell too.
    @"
@echo off
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0wilai.ps1" %*
"@ | Set-Content -LiteralPath (Join-Path $InstallTo 'wilai.cmd') -Encoding ASCII
} else {
    throw "neither wilai.exe nor wilai.ps1 found next to install.ps1"
}

if (Add-PathIfMissing $InstallTo) {
    Write-Step "added $InstallTo to your PATH (open a new terminal to pick it up)"
}

# --- config bootstrap --------------------------------------------------

New-Item -ItemType Directory -Force -Path $CfgDir | Out-Null

if (-not (Test-Path $CfgFile)) {
    if ($Mode -eq 'ask') {
        Write-Host ""
        Write-Host "How do you want to reach Wilai?"
        Write-Host "  1) WSL  — a Linux distro on this machine (Wilai installed inside)"
        Write-Host "  2) SSH  — a remote Linux box (Wilai installed there)"
        $pick = Read-Host "Pick 1 or 2"
        $Mode = switch ($pick.Trim()) { '2' { 'ssh' } default { 'wsl' } }
    }
    switch ($Mode) {
        'wsl' {
            if (-not $Distro) { $Distro = Read-Host "WSL distro name (default: WilOS)" }
            if (-not $Distro) { $Distro = 'WilOS' }
            @"
# Wilai Windows bridge — $(Get-Date -Format o)
[bridge]
mode = "wsl"

[bridge.wsl]
distro = "$Distro"
user   = ""
shell  = "wilai-shell"

[bridge.ssh]
host = ""
port = 22
key  = ""
"@ | Set-Content -LiteralPath $CfgFile -Encoding UTF8
        }
        'ssh' {
            if (-not $Host) { $Host = Read-Host "SSH target (e.g. wilai@my-box.lan)" }
            @"
# Wilai Windows bridge — $(Get-Date -Format o)
[bridge]
mode = "ssh"

[bridge.wsl]
distro = ""
user   = ""
shell  = "wilai-shell"

[bridge.ssh]
host = "$Host"
port = 22
key  = ""
"@ | Set-Content -LiteralPath $CfgFile -Encoding UTF8
        }
    }
    Write-Step "wrote $CfgFile"
} else {
    Write-Step "kept existing $CfgFile"
}

Write-Host ""
Write-Host "Wilai Windows bridge installed." -ForegroundColor Green
Write-Host ""
Write-Host "Try:"
Write-Host "   wilai              # open the TUI through the configured bridge"
Write-Host "   wilai --config     # print the resolved config"
Write-Host "   wilai --help       # commands + options"
Write-Host ""
Write-Host "To uninstall:"
Write-Host "   .\install.ps1 -Uninstall"

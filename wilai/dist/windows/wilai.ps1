# wilai.ps1 — Windows-side launcher for Wilai.
#
# Wilai (the agent) runs on Linux. This launcher is a thin bridge
# that dispatches to a Linux Wilai instance via one of two modes:
#
#   wsl  — exec wilai-shell inside a configured WSL2 distro
#   ssh  — exec wilai-shell on a configured remote host via OpenSSH
#
# Both are stock Windows components (WSL2 since Win10 2004, OpenSSH
# client since Win10 1809). No service, no tray — this file replaces
# the agent on Windows; the real daemon stays on the Linux side.
#
# Config: %APPDATA%\wilai\config.toml   (created by install.ps1).
#
# Usage:
#   wilai                      → open the TUI through the configured bridge
#   wilai --mode wsl           → force WSL mode (overrides config)
#   wilai --mode ssh           → force SSH mode (overrides config)
#   wilai --config             → print the resolved config and exit
#   wilai --help               → show this card

[CmdletBinding()]
param(
    [string]$Mode,
    [switch]$ConfigDump,
    [switch]$HelpFlag,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = 'Stop'

function Write-Err([string]$msg) {
    [Console]::Error.WriteLine("wilai: $msg")
}

function Show-Help {
    @"
Wilai — Windows bridge launcher

The agent runs on Linux. This script forwards you to a Wilai instance
via one of two bridges:

  wsl  — the configured WSL distro (wsl -d <distro> -- wilai-shell ...)
  ssh  — the configured SSH target  (ssh wilai@<host>)

Usage:
  wilai                 open the TUI through the configured bridge
  wilai --mode wsl      force WSL mode this run
  wilai --mode ssh      force SSH mode this run
  wilai --config        print the resolved config
  wilai --help          show this message

Edit %APPDATA%\wilai\config.toml to change defaults. Example:

  [bridge]
  mode = "ssh"             # or "wsl"

  [bridge.wsl]
  distro = "WilOS"         # or "Ubuntu", "Arch", etc.
  user   = ""              # empty = the distro's default user
  shell  = "wilai-shell"   # command to exec inside WSL

  [bridge.ssh]
  host   = "wilai@my-box.lan"
  port   = 22
  key    = ""              # extra -i path if you don't use default
"@
}

if ($HelpFlag) { Show-Help; return }

# --- config loading -----------------------------------------------------

$AppData = [Environment]::GetFolderPath('ApplicationData')
$CfgPath = Join-Path (Join-Path $AppData 'wilai') 'config.toml'

function Read-Toml([string]$path) {
    # Minimal TOML reader: we only need [bridge], [bridge.wsl],
    # [bridge.ssh] with scalar keys. Zero deps.
    $cfg = @{
        bridge      = @{ mode = 'wsl' }
        'bridge.wsl' = @{ distro = ''; user = ''; shell = 'wilai-shell' }
        'bridge.ssh' = @{ host = ''; port = 22; key = '' }
    }
    if (-not (Test-Path $path)) { return $cfg }
    $section = ''
    foreach ($raw in Get-Content -LiteralPath $path) {
        $line = $raw.Trim()
        if ($line -eq '' -or $line.StartsWith('#')) { continue }
        if ($line.StartsWith('[') -and $line.EndsWith(']')) {
            $section = $line.Substring(1, $line.Length - 2).Trim()
            if (-not $cfg.ContainsKey($section)) { $cfg[$section] = @{} }
            continue
        }
        if ($section -eq '') { continue }
        $eq = $line.IndexOf('=')
        if ($eq -lt 1) { continue }
        $k = $line.Substring(0, $eq).Trim()
        $v = $line.Substring($eq + 1).Trim()
        if ($v.StartsWith('"') -and $v.EndsWith('"')) {
            $v = $v.Substring(1, $v.Length - 2)
        } elseif ($v -match '^-?\d+$') {
            $v = [int]$v
        }
        $cfg[$section][$k] = $v
    }
    return $cfg
}

$cfg = Read-Toml $CfgPath

if ($ConfigDump) {
    Write-Host "config file: $CfgPath"
    $cfg.Keys | Sort-Object | ForEach-Object {
        Write-Host "[$_]"
        $cfg[$_].GetEnumerator() | Sort-Object Name | ForEach-Object {
            Write-Host ("  {0} = {1}" -f $_.Key, $_.Value)
        }
    }
    return
}

$resolvedMode = if ($Mode) { $Mode } else { [string]$cfg['bridge']['mode'] }
if (-not $resolvedMode) { $resolvedMode = 'wsl' }

# --- WSL bridge --------------------------------------------------------

function Invoke-WslBridge($bridgeCfg, $extraArgs) {
    $wsl = Get-Command wsl.exe -ErrorAction SilentlyContinue
    if (-not $wsl) {
        Write-Err "wsl.exe not found. Install WSL2: https://aka.ms/wsl"
        exit 2
    }
    $distro = [string]$bridgeCfg['distro']
    $user   = [string]$bridgeCfg['user']
    $shell  = [string]$bridgeCfg['shell']
    if (-not $shell) { $shell = 'wilai-shell' }

    $wslArgs = @()
    if ($distro) { $wslArgs += @('-d', $distro) }
    if ($user)   { $wslArgs += @('-u', $user) }
    $wslArgs += @('--')
    $wslArgs += $shell
    if ($extraArgs) { $wslArgs += $extraArgs }

    # Hand off; keep our console attached so the TUI renders inline.
    & wsl.exe @wslArgs
    exit $LASTEXITCODE
}

# --- SSH bridge --------------------------------------------------------

function Invoke-SshBridge($bridgeCfg, $extraArgs) {
    $ssh = Get-Command ssh.exe -ErrorAction SilentlyContinue
    if (-not $ssh) {
        Write-Err "ssh.exe not found. Install OpenSSH client: Settings → Apps → Optional Features → OpenSSH Client"
        exit 2
    }
    $target = [string]$bridgeCfg['host']
    if (-not $target) {
        Write-Err "no SSH host configured. Edit $CfgPath or run `wilai --config`."
        exit 2
    }
    $port = $bridgeCfg['port']
    if (-not $port) { $port = 22 }
    $key = [string]$bridgeCfg['key']

    $sshArgs = @('-t', '-o', 'ConnectTimeout=10')
    if ($port -ne 22) { $sshArgs += @('-p', $port) }
    if ($key)         { $sshArgs += @('-i', $key) }
    $sshArgs += $target

    # If the remote `wilai` user's login shell is wilai-shell (our
    # drop-shell convention from v4.3), no command needed. Otherwise
    # pass "wilai-shell" + any remaining args.
    if ($extraArgs -and $extraArgs.Count -gt 0) {
        $sshArgs += @('wilai-shell') + $extraArgs
    }

    & ssh.exe @sshArgs
    exit $LASTEXITCODE
}

# --- dispatch ----------------------------------------------------------

switch ($resolvedMode) {
    'wsl' { Invoke-WslBridge $cfg['bridge.wsl'] $Rest }
    'ssh' { Invoke-SshBridge $cfg['bridge.ssh'] $Rest }
    default {
        Write-Err "unknown bridge mode `"$resolvedMode`" (expected wsl or ssh)"
        exit 2
    }
}

<#
.SYNOPSIS
    One-shot APEX installer for Windows.

.DESCRIPTION
    Installs the Rust toolchain if needed, builds the release binary and puts it
    on your PATH. Idempotent: re-running updates the binary without touching
    your configuration, agents or task history.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File install.ps1

    # or, un-cloned:
    iwr -useb https://raw.githubusercontent.com/AdrikWashisth/project-apex/main/install.ps1 | iex
#>

[CmdletBinding()]
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\APEX",
    [string]$Repo = "https://github.com/AdrikWashisth/project-apex",
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Write-ApexLog($Message) { Write-Host "[apex] $Message" -ForegroundColor Cyan }
function Write-ApexFail($Message) { Write-Host "[apex] $Message" -ForegroundColor Red; exit 1 }

$crateDir = $PSScriptRoot

# ---------------------------------------------------------------- rust toolchain

function Ensure-Rust {
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-ApexLog "cargo found: $(cargo --version)"
        return
    }

    Write-ApexLog "cargo not found; installing via rustup"
    $rustupInit = Join-Path $env:TEMP "rustup-init.exe"
    Invoke-WebRequest -UseBasicParsing -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit
    & $rustupInit -y --no-modify-path --profile default --default-host x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { Write-ApexFail "rustup failed with exit code $LASTEXITCODE" }
    Remove-Item $rustupInit -Force

    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    Write-ApexLog "cargo installed: $(cargo --version)"
}

function Ensure-Git {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-ApexFail "git is required but was not found"
    }
}

# ---------------------------------------------------------------- build

function Build-Release {
    Write-ApexLog "building the release binary (this takes a few minutes on a cold cache)"
    Push-Location $crateDir
    try {
        cargo build --release --bin apex
        if ($LASTEXITCODE -ne 0) { Write-ApexFail "cargo build failed" }
    } finally {
        Pop-Location
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force "$crateDir\target\release\apex.exe" "$InstallDir\apex.exe"
    Write-ApexLog "installed to $InstallDir\apex.exe"
}

# ---------------------------------------------------------------- PATH

function Ensure-Path {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries -contains $InstallDir) { return }

    Write-ApexLog "adding $InstallDir to your user PATH"
    [Environment]::SetEnvironmentVariable('Path', (($entries + $InstallDir) -join ';'), 'User')
    $env:PATH = "$InstallDir;$env:PATH"
}

# ---------------------------------------------------------------- main

if (-not (Test-Path (Join-Path $crateDir "Cargo.toml"))) {
    Write-ApexFail "run this from the repository, or via the iwr one-liner"
}

Ensure-Rust
Ensure-Git
if (-not $SkipBuild) { Build-Release }
Ensure-Path

$apex = Join-Path $InstallDir "apex.exe"
& $apex --version

Write-Host @"

APEX is installed. Next steps:

  1. Open a new terminal so PATH takes effect.
  2. Check the environment:      apex doctor
  3. Point it at a model:         apex models set openai/gpt-4o-mini
     (then: $env:OPENAI_API_KEY = 'sk-...')
  4. Run a task against a repo:   cd ~/code/my-project
                                  apex run "Add input validation to user creation"

See docs/ in the repository for the architecture, roadmap and security model.
"@ -ForegroundColor White

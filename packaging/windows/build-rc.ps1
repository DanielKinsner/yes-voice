[CmdletBinding()]
param(
    [string]$Version = "1.0.0-rc.3",
    [string]$ModelBundleDir = "target\model-bundle",
    [string]$RuntimeBundleDir = "target\runtime-bundle",
    [string]$IsccPath = "",
    [switch]$SkipGate,
    [switch]$SkipSmoke
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Push-Location $repoRoot
try {
    $dirty = (& git status --porcelain | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "git status failed"
    }
    if ($dirty) {
        throw "release packaging requires a clean worktree; commit or remove these changes first:`n$dirty"
    }

    $modelPath = (Resolve-Path -LiteralPath $ModelBundleDir).Path
    $runtimePath = (Resolve-Path -LiteralPath $RuntimeBundleDir).Path
    $env:YESVOX_MODEL_BUNDLE_DIR = $modelPath
    $env:YESVOX_RUNTIME_BUNDLE_DIR = $runtimePath

    & cargo xtask package-audit
    if ($LASTEXITCODE -ne 0) {
        throw "cargo xtask package-audit failed"
    }

    if (-not $SkipGate) {
        & cargo xtask gate
        if ($LASTEXITCODE -ne 0) {
            throw "cargo xtask gate failed"
        }
    }

    & cargo build --release -p yesvox-gui -p yesvox-app --features neural
    if ($LASTEXITCODE -ne 0) {
        throw "neural release build failed"
    }

    $commit = (& git rev-parse --short=7 HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $commit) {
        throw "could not resolve the release commit"
    }
    $cliPath = (Resolve-Path "target\release\yesvox.exe").Path
    $versionLine = (& $cliPath --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "release CLI --version failed: $versionLine"
    }
    $expectedVersionLine = "yesvox $Version ($commit)"
    if ($versionLine -ne $expectedVersionLine) {
        throw "release version mismatch: expected '$expectedVersionLine', got '$versionLine'"
    }

    if (-not $IsccPath) {
        if ($env:YESVOX_ISCC) {
            $IsccPath = $env:YESVOX_ISCC
        } else {
            $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
            if ($command) {
                $IsccPath = $command.Source
            } else {
                $localIscc = Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"
                if (Test-Path -LiteralPath $localIscc) {
                    $IsccPath = $localIscc
                }
            }
        }
    }
    if (-not $IsccPath -or -not (Test-Path -LiteralPath $IsccPath)) {
        throw "ISCC.exe was not found; set -IsccPath or YESVOX_ISCC"
    }

    $env:YESVOX_PACKAGE_VERSION = $Version
    $env:YESVOX_BIN_DIR = (Resolve-Path "target\release").Path
    $env:YESVOX_RUNTIME_BUNDLE_DIR = $runtimePath
    $env:YESVOX_MODEL_BUNDLE_DIR = $modelPath
    & $IsccPath "packaging\windows\yesvox.iss"
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC.exe failed"
    }

    $installer = (Resolve-Path "target\package\yesvox-v1-unsigned.exe").Path
    $installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "package: $installer"
    Write-Host "package SHA-256: $installerHash"

    if (-not $SkipSmoke) {
        & (Join-Path $PSScriptRoot "smoke-installer.ps1") `
            -InstallerPath $installer `
            -ExpectedVersion $Version `
            -ExpectedHash $commit
        if ($LASTEXITCODE -ne 0) {
            throw "installer smoke failed"
        }
    }
} finally {
    Pop-Location
}

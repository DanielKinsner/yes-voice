[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,
    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,
    [Parameter(Mandatory = $true)]
    [string]$ExpectedHash
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-HiddenProcess {
    param(
        [Parameter(Mandatory = $true)] [string]$FilePath,
        [Parameter(Mandatory = $true)] [string[]]$Arguments
    )
    $process = Start-Process -FilePath $FilePath `
        -ArgumentList $Arguments `
        -Wait `
        -PassThru `
        -WindowStyle Hidden
    if ($process.ExitCode -ne 0) {
        throw "'$FilePath' exited $($process.ExitCode)"
    }
}

function Write-SmokeWav {
    param([Parameter(Mandatory = $true)] [string]$Path)
    $sampleRate = 48000
    $seconds = 2
    $samples = $sampleRate * $seconds
    $dataBytes = $samples * 2
    $stream = [System.IO.File]::Create($Path)
    try {
        $writer = [System.IO.BinaryWriter]::new($stream)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("RIFF"))
        $writer.Write([int](36 + $dataBytes))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("WAVE"))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("fmt "))
        $writer.Write([int]16)
        $writer.Write([int16]1)
        $writer.Write([int16]1)
        $writer.Write([int]$sampleRate)
        $writer.Write([int]($sampleRate * 2))
        $writer.Write([int16]2)
        $writer.Write([int16]16)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("data"))
        $writer.Write([int]$dataBytes)
        for ($i = 0; $i -lt $samples; $i++) {
            $t = $i / [double]$sampleRate
            $value = 0.24 * [Math]::Sin(2 * [Math]::PI * 220 * $t) +
                0.08 * [Math]::Sin(2 * [Math]::PI * 440 * $t) +
                0.03 * [Math]::Sin(2 * [Math]::PI * 660 * $t)
            $writer.Write([int16]([Math]::Round($value * 32767)))
        }
        $writer.Flush()
    } finally {
        $stream.Dispose()
    }
}

function Read-WavShape {
    param([Parameter(Mandatory = $true)] [string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        if ([System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4)) -ne "RIFF") {
            throw "$Path is not RIFF"
        }
        [void]$reader.ReadUInt32()
        if ([System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4)) -ne "WAVE") {
            throw "$Path is not WAVE"
        }
        $channels = 0
        $sampleRate = 0
        $blockAlign = 0
        $dataBytes = 0
        while ($stream.Position + 8 -le $stream.Length) {
            $chunk = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
            $size = $reader.ReadUInt32()
            $start = $stream.Position
            if ($chunk -eq "fmt ") {
                [void]$reader.ReadUInt16()
                $channels = $reader.ReadUInt16()
                $sampleRate = $reader.ReadUInt32()
                [void]$reader.ReadUInt32()
                $blockAlign = $reader.ReadUInt16()
            } elseif ($chunk -eq "data") {
                $dataBytes = $size
            }
            $stream.Position = $start + $size + ($size % 2)
        }
        if ($channels -le 0 -or $sampleRate -le 0 -or $blockAlign -le 0 -or $dataBytes -le 0) {
            throw "$Path has an incomplete WAV shape"
        }
        [pscustomobject]@{
            Channels = $channels
            SampleRate = $sampleRate
            Frames = [int64]($dataBytes / $blockAlign)
        }
    } finally {
        $stream.Dispose()
    }
}

$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$packageDir = Split-Path -Parent $installer
$smokeRoot = Join-Path $packageDir ("smoke-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
$installDir = Join-Path $smokeRoot "install"
New-Item -ItemType Directory -Path $smokeRoot | Out-Null

$resolvedSmokeRoot = [System.IO.Path]::GetFullPath($smokeRoot).TrimEnd('\') + '\'
$resolvedInstallDir = [System.IO.Path]::GetFullPath($installDir)
if (-not $resolvedInstallDir.StartsWith($resolvedSmokeRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "refusing smoke install outside the isolated smoke directory: $resolvedInstallDir"
}

$uninstaller = Join-Path $installDir "unins000.exe"
$reportPath = Join-Path $smokeRoot "smoke-report.json"
$installed = $false
$failure = $null
try {
    Invoke-HiddenProcess -FilePath $installer -Arguments @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/NOICONS",
        ('/DIR="{0}"' -f $installDir)
    )
    $installed = $true

    foreach ($relative in @(
        "yesvox.exe",
        "yesvox-gui.exe",
        "onnxruntime.dll",
        "onnxruntime_providers_shared.dll",
        "ONNX_RUNTIME_LICENSE.txt",
        "ONNX_RUNTIME_THIRD_PARTY_NOTICES.txt",
        "models\mossformer2_se_48k.onnx"
    )) {
        $required = Join-Path $installDir $relative
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "installed payload is missing $relative"
        }
    }

    $cli = Join-Path $installDir "yesvox.exe"
    $versionLine = (& $cli --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "installed yesvox --version failed: $versionLine"
    }
    $expectedVersionLine = "yesvox $ExpectedVersion ($ExpectedHash)"
    if ($versionLine -ne $expectedVersionLine) {
        throw "installed version mismatch: expected '$expectedVersionLine', got '$versionLine'"
    }

    $inputPath = Join-Path $smokeRoot "smoke-input.wav"
    $outputPath = Join-Path $smokeRoot "smoke-output.wav"
    Write-SmokeWav -Path $inputPath
    $renderLog = (& $cli $inputPath $outputPath 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "installed neural render failed: $renderLog"
    }
    if (-not $renderLog.Contains("denoiser Neural")) {
        throw "installed render did not use the bundled neural engine: $renderLog"
    }
    if (-not (Test-Path -LiteralPath $outputPath -PathType Leaf)) {
        throw "installed render produced no output"
    }
    $inputShape = Read-WavShape -Path $inputPath
    $outputShape = Read-WavShape -Path $outputPath
    if ($inputShape.Channels -ne $outputShape.Channels -or
        $inputShape.SampleRate -ne $outputShape.SampleRate -or
        $inputShape.Frames -ne $outputShape.Frames) {
        throw "installed render changed WAV shape: input=$($inputShape | ConvertTo-Json -Compress), output=$($outputShape | ConvertTo-Json -Compress)"
    }

} catch {
    $failure = $_
} finally {
    if ($installed -and (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        try {
            Invoke-HiddenProcess -FilePath $uninstaller -Arguments @(
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART"
            )
        } catch {
            if (-not $failure) {
                $failure = $_
            }
        }
    }
}

foreach ($residue in @("yesvox.exe", "yesvox-gui.exe", "onnxruntime.dll", "models\mossformer2_se_48k.onnx")) {
    if (Test-Path -LiteralPath (Join-Path $installDir $residue)) {
        throw "uninstall left release payload behind: $residue"
    }
}
if ($failure) {
    throw $failure
}

[pscustomobject]@{
    Status = "green"
    TimestampUtc = [DateTime]::UtcNow.ToString("o")
    Installer = $installer
    InstallerSha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    Version = $versionLine
    NeuralRender = $true
    Channels = $outputShape.Channels
    SampleRate = $outputShape.SampleRate
    Frames = $outputShape.Frames
    RenderLog = $renderLog
    Uninstalled = $true
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $reportPath -Encoding utf8

Write-Host "installer smoke: green"
Write-Host "report: $reportPath"

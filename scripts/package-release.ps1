param(
    [switch]$IncludeWhisper,
    [switch]$SkipBuild,
    [switch]$Offline,
    [string]$Version = "0.2.0"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

if (-not $SkipBuild) {
    Write-Host "==> Building SelaBar v$Version (release)..." -ForegroundColor Cyan
    if ($Offline) {
        cargo build --release --bin SelaBar --offline
    } else {
        cargo build --release --bin SelaBar
    }
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release --bin SelaBar gagal (exit $LASTEXITCODE)"
    }
}

$ExeSrc = Join-Path $Root "target\release\SelaBar.exe"
if (-not (Test-Path $ExeSrc)) {
    throw "Binary release tidak ditemukan: $ExeSrc"
}

$Dist = Join-Path $Root "dist"
New-Item -ItemType Directory -Path $Dist -Force | Out-Null

$ExeDist = Join-Path $Dist "SelaBar.exe"
Copy-Item -Path $ExeSrc -Destination $ExeDist -Force

# Compress SelaBar.exe into target\release\SelaBar.exe.gz for embedding in SelaBar-Setup.exe
$GzPath = Join-Path $Root "target\release\SelaBar.exe.gz"
Write-Host "==> Compressing payload for SelaBar-Setup.exe..." -ForegroundColor Cyan
$inStream = [System.IO.File]::OpenRead($ExeSrc)
try {
    $outStream = [System.IO.File]::Create($GzPath)
    try {
        $gzStream = New-Object System.IO.Compression.GZipStream(
            $outStream,
            [System.IO.Compression.CompressionLevel]::Optimal
        )
        try {
            $inStream.CopyTo($gzStream)
        } finally {
            $gzStream.Dispose()
        }
    } finally {
        $outStream.Dispose()
    }
} finally {
    $inStream.Dispose()
}

Write-Host "==> Building SelaBar-Setup v$Version (release)..." -ForegroundColor Cyan
if ($Offline) {
    cargo build --release --bin SelaBar-Setup --features setup-payload --offline
} else {
    cargo build --release --bin SelaBar-Setup --features setup-payload
}
if ($LASTEXITCODE -ne 0) {
    throw "cargo build --release --bin SelaBar-Setup --features setup-payload gagal (exit $LASTEXITCODE)"
}

$SetupSrc = Join-Path $Root "target\release\SelaBar-Setup.exe"
$SetupDist = Join-Path $Dist "SelaBar-v$Version-Setup.exe"
Copy-Item -Path $SetupSrc -Destination $SetupDist -Force

$InstallNotePath = Join-Path $Dist "README-INSTALL.txt"
$InstallNote = @"
SelaBar v$Version — Dynamic Notch & Desktop AI Companion (Windows x64)
========================================================================

Cara Install:
- Rekomendasi: Jalankan SelaBar-v$Version-Setup.exe (Installer Windows resmi).
  Otomatis memasang ke %LOCALAPPDATA%\Programs\SelaBar, membuat shortcut Start Menu
  & Desktop, serta mendaftarkan SelaBar di Windows Settings -> Apps untuk uninstall bersih.
- Portable: Jalankan SelaBar.exe langsung (otomatis mendaftarkan shortcut & Apps Uninstall).

Suara Balasan AI (TTS):
- Default memakai Pure Rust Edge Neural TTS (Gadis / Ardi) dengan tuning emosi & EQ,
  lalu otomatis fallback ke suara offline Windows bila tanpa internet.
- Untuk memilih suara utama (Neural / Offline), buka tab Settings -> Voice -> Suara Utama.

Suara Input Mikrofon / Live Voice AI (Whisper STT):
- Tekan Ctrl+Alt+V (Live Voice AI), Ctrl+Alt+D (Dikte ke Kursor), atau klik ikon Mic.
- Pilih CPU (~190 MB) untuk laptop tanpa GPU NVIDIA atau CUDA (~882 MB) untuk GPU NVIDIA.
- Paket runtime & model tersimpan di %APPDATA%\SelaBar\.

AI Providers (Auto-Fallback):
- Buka Settings -> General untuk mengisi Google Gemini (AI Studio API Key),
  OpenAI-Compatible (Endpoint URL + API Key), dan/atau Local/Custom Router.
- Jika satu provider terkena rate limit (429) atau error, SelaBar otomatis pindah ke provider lain.

Hotkey Default:
- Ctrl+Alt+V : Live Voice AI (Bicara terus-menerus)
- Ctrl+Alt+D : Dikte suara langsung ke kursor aktif
- Ctrl+Alt+C : Ask AI popup cepat
- Alt+C      : Buka Tab Chat AI
- Esc        : Hentikan / Tutup overlay aktif
"@
Set-Content -Path $InstallNotePath -Value $InstallNote -Encoding UTF8

$PortableZip = Join-Path $Dist "SelaBar-v$Version-win64-portable.zip"
if (Test-Path $PortableZip) {
    Remove-Item $PortableZip -Force
}
Compress-Archive -Path @($ExeDist, $InstallNotePath) -DestinationPath $PortableZip -CompressionLevel Optimal -Force

if ($IncludeWhisper) {
    $CudaDir = Join-Path $Root "whisper-cuda"
    $ModelSrc = Join-Path $Root "desknotch-models\ggml-small.bin"
    if (-not (Test-Path $ModelSrc)) {
        $ModelSrc = Join-Path $Root "models\ggml-small-q5_1.bin"
    }
    if ((Test-Path $CudaDir) -and (Test-Path $ModelSrc)) {
        $StageDir = Join-Path $Dist "_full_stage"
        if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }
        New-Item -ItemType Directory -Path (Join-Path $StageDir "whisper-cuda") -Force | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $StageDir "models") -Force | Out-Null

        Copy-Item $ExeDist -Destination (Join-Path $StageDir "SelaBar.exe") -Force
        Copy-Item $SetupDist -Destination (Join-Path $StageDir "SelaBar-v$Version-Setup.exe") -Force
        Copy-Item $InstallNotePath -Destination (Join-Path $StageDir "README-INSTALL.txt") -Force
        Copy-Item (Join-Path $CudaDir "*") -Destination (Join-Path $StageDir "whisper-cuda") -Recurse -Force
        Copy-Item $ModelSrc -Destination (Join-Path $StageDir "models\$(Split-Path -Leaf $ModelSrc)") -Force

        $FullZip = Join-Path $Dist "SelaBar-v$Version-win64-full-cuda.zip"
        if (Test-Path $FullZip) { Remove-Item $FullZip -Force }
        Compress-Archive -Path (Join-Path $StageDir "*") -DestinationPath $FullZip -CompressionLevel Optimal -Force
        Remove-Item $StageDir -Recurse -Force
    } else {
        Write-Warning "Melewati bundle -IncludeWhisper karena whisper-cuda/ atau model tidak ditemukan."
    }
}

Remove-Item $InstallNotePath -Force -ErrorAction SilentlyContinue

Write-Host "`n==> Artefak Rilis SelaBar v$Version siap di folder dist\:" -ForegroundColor Green
Get-ChildItem $Dist -File | ForEach-Object {
    $hash = (Get-FileHash -Path $_.FullName -Algorithm SHA256).Hash
    $mb = [math]::Round($_.Length / 1MB, 2)
    Write-Host ("  - {0} ({1} MB)  SHA256: {2}" -f $_.Name, $mb, $hash)
}

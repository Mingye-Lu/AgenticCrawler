#Requires -Version 5.1
<#
.SYNOPSIS
    Install acrawl on Windows (PowerShell 5.1+).

.DESCRIPTION
    Downloads the latest acrawl binary from GitHub Releases, verifies the
    SHA256 checksum, installs to $HOME\.acrawl\bin\, adds to user PATH,
    and optionally sets up Playwright for browser automation.

.EXAMPLE
    irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex
#>

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "Mingye-Lu/AgenticCrawler"
if ($env:ACRAWL_CONFIG_HOME) {
    $ConfigHome = $env:ACRAWL_CONFIG_HOME
} else {
    $ConfigHome = Join-Path $HOME ".acrawl"
}
$InstallDir = Join-Path $ConfigHome "bin"

# --- 1. Architecture check ---
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64") {
    Write-Error "Unsupported architecture: $arch. Only AMD64 (x64) Windows is supported."
    exit 1
}

Write-Host ""
Write-Host "  acrawl installer for Windows" -ForegroundColor Cyan
Write-Host "  =============================" -ForegroundColor Cyan
Write-Host ""

# --- 2. Get latest version from GitHub API ---
Write-Host "Fetching latest release..." -ForegroundColor Gray
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
    $version = $release.tag_name.TrimStart('v')
} catch {
    Write-Error "Failed to fetch latest release from GitHub: $_"
    exit 1
}

Write-Host "  Latest version: v$version" -ForegroundColor Green

# --- 3. Download binary ---
$binaryUrl = "https://github.com/$Repo/releases/download/v$version/acrawl-windows-x64.exe"
$tempBinary = Join-Path $env:TEMP "acrawl-download.exe"

Write-Host "Downloading acrawl v$version..." -ForegroundColor Gray
try {
    Invoke-WebRequest -Uri $binaryUrl -OutFile $tempBinary -UseBasicParsing
} catch {
    Write-Error "Failed to download binary from: $binaryUrl`n$_"
    exit 1
}

# --- 4. Download checksums ---
$checksumUrl = "https://github.com/$Repo/releases/download/v$version/checksums.sha256"
$checksumFile = Join-Path $env:TEMP "acrawl-checksums.sha256"

Write-Host "Downloading checksums..." -ForegroundColor Gray
try {
    Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumFile -UseBasicParsing
} catch {
    Write-Error "Failed to download checksums from: $checksumUrl`n$_"
    exit 1
}

# --- 5. Verify SHA256 checksum ---
Write-Host "Verifying checksum..." -ForegroundColor Gray
$actualHash = (Get-FileHash -Path $tempBinary -Algorithm SHA256).Hash.ToLower()
$checksumContent = Get-Content $checksumFile
$expectedLine = $checksumContent | Where-Object { $_ -match "acrawl-windows-x64\.exe" }

if (-not $expectedLine) {
    Write-Error "Could not find checksum for acrawl-windows-x64.exe in checksums file."
    exit 1
}

$expectedHash = ($expectedLine -split '\s+')[0].ToLower()

if ($actualHash -ne $expectedHash) {
    Write-Error "Checksum verification FAILED!`n  Expected: $expectedHash`n  Got:      $actualHash`nThe downloaded file may be corrupted or tampered with."
    exit 1
}

Write-Host "  Checksum verified." -ForegroundColor Green

# --- 6. Create install directory ---
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

# --- 7. Install binary ---
$targetPath = Join-Path $InstallDir "acrawl.exe"
Move-Item -Path $tempBinary -Destination $targetPath -Force
Write-Host "  Installed to: $targetPath" -ForegroundColor Green

# --- 8. Add to user PATH ---
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $userPath) {
    $userPath = ""
}

if ($userPath -notlike "*$InstallDir*") {
    if ($userPath -and -not $userPath.EndsWith(";")) {
        $newPath = "$userPath;$InstallDir"
    } else {
        $newPath = "$userPath$InstallDir"
    }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host ""
    Write-Host "  Added $InstallDir to user PATH." -ForegroundColor Yellow
    Write-Host "  Restart your terminal for PATH changes to take effect." -ForegroundColor Yellow
} else {
    Write-Host "  $InstallDir already in PATH." -ForegroundColor Gray
}

# --- 9. Node.js check ---
Write-Host ""
$nodeAvailable = $false
$nodeMajor = 0

try {
    $nodeVersionRaw = & node --version 2>$null
    if ($nodeVersionRaw) {
        $nodeVersion = $nodeVersionRaw.TrimStart('v')
        $nodeMajor = [int]($nodeVersion -split '\.')[0]
        if ($nodeMajor -lt 16) {
            Write-Warning "Node.js 16+ is required for browser automation. You have v$nodeVersion."
            Write-Warning "Install from https://nodejs.org/ to enable headless browser features."
        } else {
            $nodeAvailable = $true
            Write-Host "  Node.js v$nodeVersion detected." -ForegroundColor Green
        }
    } else {
        Write-Warning "Node.js not found. Browser automation requires Node.js 16+."
        Write-Warning "Install from https://nodejs.org/ to enable headless browser features."
    }
} catch {
    Write-Warning "Node.js not found. Browser automation requires Node.js 16+."
    Write-Warning "Install from https://nodejs.org/ to enable headless browser features."
}

# --- 10. Playwright install ---
if ($nodeAvailable) {
    $playwrightDir = Join-Path $ConfigHome "node_modules\playwright"
    if (Test-Path $playwrightDir) {
        Write-Host "  Playwright already installed." -ForegroundColor Gray
    } else {
        Write-Host "Installing Playwright..." -ForegroundColor Gray
        try {
            & npm install --prefix $ConfigHome playwright 2>&1 | Out-Null
            & npx --prefix $ConfigHome playwright install chromium 2>&1 | Out-Null
            Write-Host "  Playwright + Chromium installed." -ForegroundColor Green
        } catch {
            Write-Warning "Playwright installation failed: $_"
            Write-Warning "You can install it manually later: npm install --prefix `"$ConfigHome`" playwright && npx --prefix `"$ConfigHome`" playwright install chromium"
        }
    }
}

# --- 11. Cleanup temp files ---
if (Test-Path $checksumFile) {
    Remove-Item -Path $checksumFile -Force -ErrorAction SilentlyContinue
}

# --- 12. Success ---
Write-Host ""
Write-Host "  acrawl v$version installed successfully!" -ForegroundColor Green
Write-Host ""
Write-Host "  Get started:" -ForegroundColor Cyan
Write-Host "    acrawl auth anthropic    # configure your LLM provider"
Write-Host "    acrawl                   # launch interactive REPL"
Write-Host ""

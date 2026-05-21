$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -LiteralPath $MyInvocation.MyCommand.Path
Push-Location $scriptDir
try {
    if (Test-Path 'extension.zip') { Remove-Item 'extension.zip' }
    $paths = @(
        'manifest.json',
        'background.js',
        'options.html',
        'options.js',
        'icons',
        'commands'
    )
    Compress-Archive -Path $paths -DestinationPath 'extension.zip' -Force
    Write-Host "Built: $scriptDir\extension.zip"
} finally {
    Pop-Location
}

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -LiteralPath $MyInvocation.MyCommand.Path
Push-Location $scriptDir
try {
    if (Test-Path 'extension.zip') { Remove-Item 'extension.zip' }
    $exclude = @('*.zip', '*.ps1', '*.sh', '.DS_Store', 'PRIVACY.md', 'README.md')
    $files = Get-ChildItem -Recurse -File |
        Where-Object { $name = $_.Name; -not ($exclude | Where-Object { $name -like $_ }) }
    Compress-Archive -Path $files.FullName -DestinationPath 'extension.zip' -Force
    Write-Host "Built: $scriptDir\extension.zip"
} finally {
    Pop-Location
}

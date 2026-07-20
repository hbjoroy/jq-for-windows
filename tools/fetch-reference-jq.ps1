param(
    [string]$Destination = "$PSScriptRoot\reference\jq-1.7.1-windows-amd64.exe"
)

$ErrorActionPreference = 'Stop'
$version = '1.7.1'
$expectedSha256 = '7451FBBF37FEFFB9BF262BD97C54F0DA558C63F0748E64152DD87B0A07B6D6AB'
$url = "https://github.com/jqlang/jq/releases/download/jq-$version/jq-windows-amd64.exe"

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Destination) | Out-Null
Invoke-WebRequest -Uri $url -OutFile $Destination
$actualSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Destination).Hash
if ($actualSha256 -ne $expectedSha256) {
    Remove-Item -LiteralPath $Destination
    throw "jq checksum mismatch: expected $expectedSha256, got $actualSha256"
}

Write-Host "Verified jq-$version at $Destination"


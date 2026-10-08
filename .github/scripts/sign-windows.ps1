# SPDX-License-Identifier: Apache-2.0
#
# Authenticode-signs the staged Windows release (docs/release/signing.md).
# Run by `cargo xtask dist` through OM_SIGN_COMMAND with the staging
# directory as its argument, only when the publisher's secrets exist:
#
#   WINDOWS_CERTIFICATE_PFX        base64 of the code-signing certificate (.pfx)
#   WINDOWS_CERTIFICATE_PASSWORD   its password
#
# Untested until the owner has a code-signing certificate. Azure Trusted
# Signing is a cheaper alternative; see the docs for how to swap it in.
param([Parameter(Mandatory = $true)][string]$Stage)
$ErrorActionPreference = "Stop"

foreach ($v in "WINDOWS_CERTIFICATE_PFX", "WINDOWS_CERTIFICATE_PASSWORD") {
    if (-not (Get-Item "env:$v" -ErrorAction SilentlyContinue)) { throw "sign-windows: $v is not set" }
}

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $signtool) { throw "sign-windows: signtool.exe not found" }

$pfx = Join-Path ([System.IO.Path]::GetTempPath()) "openmapper-signing.pfx"
try {
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE_PFX))
    $files = @((Join-Path $Stage "openmapper.exe"), (Join-Path $Stage "openmapper-cli.exe"))
    & $signtool.FullName sign /fd SHA256 /td SHA256 /tr http://timestamp.digicert.com `
        /f $pfx /p $env:WINDOWS_CERTIFICATE_PASSWORD @files
    if ($LASTEXITCODE -ne 0) { throw "sign-windows: signtool failed ($LASTEXITCODE)" }
    & $signtool.FullName verify /pa /v @files
    if ($LASTEXITCODE -ne 0) { throw "sign-windows: verification failed ($LASTEXITCODE)" }
    Write-Host "sign-windows: signed $(Split-Path $Stage -Leaf)"
} finally {
    Remove-Item $pfx -ErrorAction SilentlyContinue
}

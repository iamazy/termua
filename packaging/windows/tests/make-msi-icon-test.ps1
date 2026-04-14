$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "..\make-msi.ps1"
$scriptContent = Get-Content -Raw -Path $scriptPath
$mainIndex = $scriptContent.IndexOf('if ($env:OS -notlike "*Windows*") {')
if ($mainIndex -lt 0) {
  throw "Failed to locate make-msi.ps1 main entrypoint"
}

$functionsOnly = $scriptContent.Substring(0, $mainIndex)
Invoke-Expression $functionsOnly

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("termua-msi-icon-test-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path (Join-Path $tmpRoot "assets\logo") | Out-Null

$repoIco = Join-Path $tmpRoot "assets\logo\termua.ico"
[System.IO.File]::WriteAllBytes($repoIco, [byte[]](0, 1, 2, 3))

try {
  $resolved = Ensure-TermuaIco $tmpRoot "x86_64"
  if ($resolved -ne (Resolve-Path $repoIco).Path) {
    throw "Expected Ensure-TermuaIco to prefer checked-in repo icon. Got: $resolved"
  }
  Write-Host "PASS: Ensure-TermuaIco prefers repo .ico"

  Remove-Item -Force $repoIco
  $repoSvg = Join-Path $tmpRoot "assets\logo\termua.svg"
  @'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect width="16" height="16" fill="#000"/>
</svg>
'@ | Set-Content -Path $repoSvg -NoNewline

  $resolved = Ensure-TermuaIco $tmpRoot "x86_64"
  if ($null -ne $resolved) {
    throw "Expected Ensure-TermuaIco to ignore svg fallback and return null. Got: $resolved"
  }
  Write-Host "PASS: Ensure-TermuaIco ignores svg fallback"

  New-Item -ItemType Directory -Force -Path (Join-Path $tmpRoot "wix") | Out-Null
  $wxsPath = Join-Path $tmpRoot "wix\main.wxs"
  @'
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Product Id="*" Name="termua" Language="1033" Version="0.1.0" Manufacturer="termua" UpgradeCode="PUT-GUID-HERE">
  </Product>
</Wix>
'@ | Set-Content -Path $wxsPath -NoNewline

  [System.IO.File]::WriteAllBytes($repoIco, [byte[]](0, 1, 2, 3))
  Ensure-WixIcon $tmpRoot $repoIco

  $copiedIco = Join-Path $tmpRoot "wix\termua.ico"
  if (-not (Test-Path $copiedIco)) {
    throw "Expected Ensure-WixIcon to copy icon into wix directory. Missing: $copiedIco"
  }

  $updated = Get-Content -Raw -Path $wxsPath
  $expectedIconPath = [regex]::Escape((Resolve-Path $copiedIco).Path)
  if ($updated -notmatch ('<Icon Id="termuaIcon" SourceFile="' + $expectedIconPath + '" />')) {
    throw "Expected Ensure-WixIcon to reference copied icon by full path. Got:`n$updated"
  }
  if ($updated -notmatch '<Property Id="ARPPRODUCTICON" Value="termuaIcon" />') {
    throw "Expected Ensure-WixIcon to set ARPPRODUCTICON. Got:`n$updated"
  }
  Write-Host "PASS: Ensure-WixIcon injects bindable icon path"
} finally {
  Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
}

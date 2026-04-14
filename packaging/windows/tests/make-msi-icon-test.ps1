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
} finally {
  Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
}

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "..\make-msi.ps1"
$scriptContent = Get-Content -Raw -Path $scriptPath
$mainIndex = $scriptContent.IndexOf('if ($env:OS -notlike "*Windows*") {')
if ($mainIndex -lt 0) {
  throw "Failed to locate make-msi.ps1 main entrypoint"
}

$functionsOnly = $scriptContent.Substring(0, $mainIndex)
Invoke-Expression $functionsOnly

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("termua-msi-relay-test-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path (Join-Path $tmpRoot "wix") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $tmpRoot "target\x86_64-pc-windows-msvc\release") | Out-Null

$wxsPath = Join-Path $tmpRoot "wix\main.wxs"
@'
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Product Id="*" Name="termua" Language="1033" Version="0.1.0" Manufacturer="termua" UpgradeCode="PUT-GUID-HERE">
    <Package InstallerVersion="200" Compressed="yes" />
    <Media Id="1" Cabinet="product.cab" EmbedCab="yes" />
    <Directory Id="TARGETDIR" Name="SourceDir">
      <Directory Id="ProgramFilesFolder">
        <Directory Id="INSTALLDIR" Name="termua">
          <Component Id="MainExecutable" Guid="*">
            <File Id="termuaExeFile" Name="termua.exe" Source="$(var.CargoTargetBinDir)\termua.exe" KeyPath="yes" Checksum="yes" />
          </Component>
        </Directory>
      </Directory>
    </Directory>
  </Product>
</Wix>
'@ | Set-Content -Path $wxsPath -NoNewline

$relayExe = Join-Path $tmpRoot "target\x86_64-pc-windows-msvc\release\termua-relay.exe"
[System.IO.File]::WriteAllBytes($relayExe, [byte[]](0, 1, 2, 3))

try {
  Ensure-WixRelayBinary $tmpRoot "x86_64-pc-windows-msvc"

  $updated = Get-Content -Raw -Path $wxsPath
  if ($updated -notmatch 'Source="\$\(var\.CargoTargetBinDir\)\\termua-relay\.exe"') {
    throw "Expected relay file Source to use `$(var.CargoTargetBinDir)\\termua-relay.exe`. Got:`n$updated"
  }
  if ($updated -match [regex]::Escape($relayExe)) {
    throw "Expected relay file Source to avoid absolute path. Got:`n$updated"
  }
  Write-Host "PASS: Ensure-WixRelayBinary uses CargoTargetBinDir"
} finally {
  Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
}

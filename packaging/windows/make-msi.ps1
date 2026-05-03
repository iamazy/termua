$ErrorActionPreference = "Stop"
$script:WixNamespaceUri = "http://schemas.microsoft.com/wix/2006/wi"

<#
.SYNOPSIS
  Build an MSI for termua on Windows using cargo-wix.

.REQUIRES
  - Rust toolchain (cargo)
  - WiX Toolset (candle.exe + light.exe in PATH)

.USAGE
  pwsh -NoProfile -ExecutionPolicy Bypass -File packaging/windows/make-msi.ps1
  # or Windows PowerShell:
  powershell -ExecutionPolicy Bypass -File packaging/windows/make-msi.ps1

  # Custom output directory
  $env:OUT_DIR="target\\msi"
  pwsh -NoProfile -ExecutionPolicy Bypass -File packaging/windows/make-msi.ps1

.ICON
  # Optional: override the installer icon explicitly
  # $env:ICON_ICO="assets\\logo\\termua.ico"

.ARCH / .TARGET
  $env:ARCH="x86_64"   # or "aarch64"
  # or set a full Rust target triple explicitly:
  # $env:TARGET="x86_64-pc-windows-msvc"

.NOTES
  - Installs cargo-wix automatically if missing.
  - Runs `cargo wix init` automatically if no .wxs files exist yet.
  - WiX Toolset is NOT auto-installed by default.
    If you set TERMUA_AUTO_INSTALL_WIX=1 and have winget or choco available,
    the script will attempt to install WiX.
#>

function RepoRoot {
  $here = $PSScriptRoot
  if ([string]::IsNullOrWhiteSpace($here)) {
    if (-not [string]::IsNullOrWhiteSpace($PSCommandPath)) {
      $here = Split-Path -Parent $PSCommandPath
    } elseif ($MyInvocation.MyCommand -and $MyInvocation.MyCommand.Definition) {
      $here = Split-Path -Parent $MyInvocation.MyCommand.Definition
    } else {
      throw "Unable to determine script directory (PSScriptRoot/PSCommandPath/MyInvocation are empty)."
    }
  }
  return (Resolve-Path (Join-Path $here "..\\..")).Path
}

function Ensure-InPath([string] $dir) {
  $parts = ($env:Path -split ";") | Where-Object { $_ -ne "" }
  if ($parts -notcontains $dir) {
    $env:Path = $dir + ";" + $env:Path
  }
}

function Ensure-Tool([string] $name, [string] $installHint) {
  if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
    Write-Error "Missing $name. $installHint"
  }
}

function Find-WixToolsetBin {
  $candidates = @()

  if (-not [string]::IsNullOrWhiteSpace($env:WIX)) {
    $candidates += $env:WIX
    $candidates += (Join-Path $env:WIX "bin")
  }

  $pf86 = ${env:ProgramFiles(x86)}
  if (-not [string]::IsNullOrWhiteSpace($pf86)) {
    $candidates += (Join-Path $pf86 "WiX Toolset v3.11\\bin")
    $candidates += (Join-Path $pf86 "WiX Toolset v3.14\\bin")

    $roots = @(
      (Join-Path $pf86 "WiX Toolset v3.*")
      (Join-Path $pf86 "WiX Toolset v*")
    )
    foreach ($rootGlob in $roots) {
      foreach ($dir in (Get-ChildItem -Path $rootGlob -Directory -ErrorAction SilentlyContinue)) {
        $candidates += (Join-Path $dir.FullName "bin")
      }
    }
  }

  $pf = $env:ProgramFiles
  if (-not [string]::IsNullOrWhiteSpace($pf)) {
    $roots = @(
      (Join-Path $pf "WiX Toolset v3.*")
      (Join-Path $pf "WiX Toolset v*")
    )
    foreach ($rootGlob in $roots) {
      foreach ($dir in (Get-ChildItem -Path $rootGlob -Directory -ErrorAction SilentlyContinue)) {
        $candidates += (Join-Path $dir.FullName "bin")
      }
    }
  }

  foreach ($dir in $candidates) {
    if ([string]::IsNullOrWhiteSpace($dir)) { continue }
    if (-not (Test-Path $dir)) { continue }
    if ((Test-Path (Join-Path $dir "candle.exe")) -and (Test-Path (Join-Path $dir "light.exe"))) {
      return (Resolve-Path $dir).Path
    }
  }

  return $null
}

function Ensure-WixToolsetAvailable {
  if ((Get-Command "candle.exe" -ErrorAction SilentlyContinue) -and (Get-Command "light.exe" -ErrorAction SilentlyContinue)) {
    return $true
  }

  $bin = Find-WixToolsetBin
  if ($bin) {
    Ensure-InPath $bin
  }

  return ((Get-Command "candle.exe" -ErrorAction SilentlyContinue) -and (Get-Command "light.exe" -ErrorAction SilentlyContinue))
}

function Ensure-CargoWix {
  Ensure-InPath (Join-Path $env:USERPROFILE ".cargo\\bin")
  if (-not (Get-Command "cargo-wix" -ErrorAction SilentlyContinue)) {
    Write-Host "==> Installing cargo-wix (missing)"
    & cargo install cargo-wix --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo install cargo-wix failed ($LASTEXITCODE)" }
  }
  Ensure-Tool "cargo-wix" "Try: cargo install cargo-wix --locked"
}

function Get-CargoPackageVersion([string] $packageName) {
  $metadataJson = & cargo metadata --format-version 1 --no-deps
  if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed ($LASTEXITCODE)"
  }

  $metadata = $metadataJson | ConvertFrom-Json
  $package = $metadata.packages |
    Where-Object { $_.name -eq $packageName } |
    Select-Object -First 1

  if (-not $package) {
    throw "Failed to locate package '$packageName' in cargo metadata."
  }

  return [string]$package.version
}

function Try-InstallWixToolset {
  if ($env:TERMUA_AUTO_INSTALL_WIX -ne "1") {
    return
  }

  if (Ensure-WixToolsetAvailable) { return }

  if (Get-Command "winget" -ErrorAction SilentlyContinue) {
    Write-Host "==> Installing WiX Toolset via winget (TERMUA_AUTO_INSTALL_WIX=1)"
    & winget install --id WiXToolset.WiXToolset -e --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE -ne 0) {
      throw "winget failed ($LASTEXITCODE). If it says administrator privileges are required, re-run PowerShell as Administrator or install WiX Toolset manually."
    }
    return
  }

  if (Get-Command "choco" -ErrorAction SilentlyContinue) {
    Write-Host "==> Installing WiX Toolset via choco (TERMUA_AUTO_INSTALL_WIX=1)"
    & choco install wixtoolset -y
    if ($LASTEXITCODE -ne 0) {
      throw "choco failed ($LASTEXITCODE). If it says administrator privileges are required, re-run PowerShell as Administrator or install WiX Toolset manually."
    }
    return
  }
}

function Find-LatestMsi([string] $repoRoot) {
  $candidates = @(
    Join-Path $repoRoot "target\\wix"
    Join-Path $repoRoot "termua\\target\\wix"
  )

  foreach ($dir in $candidates) {
    if (Test-Path $dir) {
      $msi = Get-ChildItem -Path $dir -Recurse -Filter "*.msi" |
        Sort-Object -Property LastWriteTime -Descending |
        Select-Object -First 1
      if ($msi) { return $msi.FullName }
    }
  }
  return $null
}

function Find-WxsFiles([string] $repoRoot) {
  $candidates = @(
    Join-Path $repoRoot "wix"
    Join-Path $repoRoot "termua\\wix"
  )

  foreach ($dir in $candidates) {
    if (Test-Path $dir) {
      $files = Get-ChildItem -Path $dir -Recurse -Filter "*.wxs" -ErrorAction SilentlyContinue
      if ($files -and $files.Count -gt 0) {
        return $files
      }
    }
  }
  return @()
}

function New-WixXmlDocument([string] $path) {
  $doc = New-Object System.Xml.XmlDocument
  $doc.PreserveWhitespace = $true
  $doc.Load($path)
  return $doc
}

function New-WixNamespaceManager([System.Xml.XmlDocument] $doc) {
  $ns = New-Object -TypeName System.Xml.XmlNamespaceManager -ArgumentList $doc.NameTable
  $null = $ns.AddNamespace("wix", $script:WixNamespaceUri)
  return ,$ns
}

function New-WixElement([System.Xml.XmlDocument] $doc, [string] $name) {
  return $doc.CreateElement($name, $script:WixNamespaceUri)
}

function Set-WixAttribute([System.Xml.XmlElement] $element, [string] $name, [string] $value) {
  if ($element.GetAttribute($name) -eq $value) {
    return $false
  }

  $element.SetAttribute($name, $value)
  return $true
}

function Save-WixXmlDocument([System.Xml.XmlDocument] $doc, [string] $path) {
  $settings = New-Object System.Xml.XmlWriterSettings
  $settings.Indent = $true
  $settings.NewLineChars = "`r`n"
  $settings.NewLineHandling = [System.Xml.NewLineHandling]::Replace
  $settings.OmitXmlDeclaration = $false
  $settings.Encoding = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false

  $writer = [System.Xml.XmlWriter]::Create($path, $settings)
  try {
    $doc.Save($writer)
  } finally {
    $writer.Dispose()
  }
}

function Invoke-WxsFileUpdate([string] $repoRoot, [scriptblock] $update) {
  $wxsFiles = Find-WxsFiles $repoRoot
  if (-not $wxsFiles -or $wxsFiles.Count -eq 0) { return }

  foreach ($file in $wxsFiles) {
    $doc = New-WixXmlDocument $file.FullName
    $ns = New-WixNamespaceManager $doc
    $changed = & $update $doc $ns $file.FullName
    if ($changed) {
      Save-WixXmlDocument $doc $file.FullName
    }
  }
}

function Add-WixComponentRefToFeature(
  [System.Xml.XmlDocument] $doc,
  [System.Xml.XmlNamespaceManager] $ns,
  [System.Xml.XmlElement] $feature,
  [string] $componentRefId
) {
  if ($feature.SelectSingleNode("wix:ComponentRef[@Id='$componentRefId']", $ns)) {
    return $false
  }

  $componentRef = New-WixElement $doc "ComponentRef"
  $null = Set-WixAttribute $componentRef "Id" $componentRefId

  $nestedFeature = $feature.SelectSingleNode("wix:Feature", $ns)
  if ($nestedFeature) {
    $null = $feature.InsertBefore($componentRef, $nestedFeature)
  } else {
    $null = $feature.AppendChild($componentRef)
  }

  return $true
}

function Get-WixAncestorContainerId([System.Xml.XmlNode] $node) {
  $current = $node.ParentNode
  while ($current) {
    if (
      ($current.LocalName -eq "Directory" -or $current.LocalName -eq "DirectoryRef") -and
      $current.Attributes["Id"]
    ) {
      return $current.Attributes["Id"].Value
    }
    $current = $current.ParentNode
  }

  return $null
}

function Ensure-TermuaIco([string] $repoRoot, [string] $arch) {
  $repoIco = Join-Path $repoRoot "assets\\logo\\termua.ico"

  $ico = $env:ICON_ICO
  if (-not [string]::IsNullOrWhiteSpace($ico)) {
    if (Test-Path $ico) {
      return (Resolve-Path $ico).Path
    }
    Write-Host "warning: ICON_ICO not found: $ico"
  }

  if (Test-Path $repoIco) {
    return (Resolve-Path $repoIco).Path
  }

  $ico = Join-Path $repoRoot ("target\\icons\\{0}\\termua.ico" -f $arch)
  if (Test-Path $ico) {
    return (Resolve-Path $ico).Path
  }
  return $null
}

function Ensure-WixIcon([string] $repoRoot, [string] $icoPath) {
  if ([string]::IsNullOrWhiteSpace($icoPath)) { return }
  if (-not (Test-Path $icoPath)) { return }

  $wxsFiles = Find-WxsFiles $repoRoot
  if (-not $wxsFiles -or $wxsFiles.Count -eq 0) { return }

  $wixDir = Split-Path -Parent $wxsFiles[0].FullName
  $destIco = Join-Path $wixDir "termua.ico"
  Copy-Item -Force $icoPath $destIco
  $iconSource = (Resolve-Path $destIco).Path

  Invoke-WxsFileUpdate $repoRoot {
    param(
      [System.Xml.XmlDocument] $doc,
      [System.Xml.XmlNamespaceManager] $ns,
      [string] $filePath
    )

    $changed = $false
    $product = $doc.SelectSingleNode("/wix:Wix/wix:Product", $ns)
    if (-not $product) { return $false }

    $icon = $product.SelectSingleNode("wix:Icon[@Id='termuaIcon']", $ns)
    if (-not $icon) {
      $icon = New-WixElement $doc "Icon"
      $null = Set-WixAttribute $icon "Id" "termuaIcon"
      $null = Set-WixAttribute $icon "SourceFile" $iconSource
      $null = $product.AppendChild($icon)
      $changed = $true
    } else {
      $changed = (Set-WixAttribute $icon "SourceFile" $iconSource) -or $changed
    }

    $arpProductIcon = $product.SelectSingleNode("wix:Property[@Id='ARPPRODUCTICON']", $ns)
    if (-not $arpProductIcon) {
      $arpProductIcon = New-WixElement $doc "Property"
      $null = Set-WixAttribute $arpProductIcon "Id" "ARPPRODUCTICON"
      $null = Set-WixAttribute $arpProductIcon "Value" "termuaIcon"
      $null = $product.AppendChild($arpProductIcon)
      $changed = $true
    } else {
      $changed = (Set-WixAttribute $arpProductIcon "Value" "termuaIcon") -or $changed
    }

    $shortcuts = $doc.SelectNodes("//wix:Shortcut", $ns)
    foreach ($shortcut in $shortcuts) {
      if ([string]::IsNullOrWhiteSpace($shortcut.GetAttribute("Icon"))) {
        $changed = (Set-WixAttribute $shortcut "Icon" "termuaIcon") -or $changed
      }
    }

    return $changed
  }
}

function Ensure-WixDesktopShortcut([string] $repoRoot) {
  Invoke-WxsFileUpdate $repoRoot {
    param(
      [System.Xml.XmlDocument] $doc,
      [System.Xml.XmlNamespaceManager] $ns,
      [string] $filePath
    )

    $changed = $false
    $product = $doc.SelectSingleNode("/wix:Wix/wix:Product", $ns)
    $targetDir = $doc.SelectSingleNode("/wix:Wix/wix:Product/wix:Directory[@Id='TARGETDIR']", $ns)
    if (-not $product -or -not $targetDir) { return $false }

    $mainFile = $doc.SelectSingleNode("//wix:File[@Name='termua.exe']", $ns)
    if (-not $mainFile) { return $false }

    $mainComponent = $mainFile.ParentNode
    if (-not $mainComponent -or $mainComponent.LocalName -ne "Component") { return $false }

    $mainFileId = $mainFile.GetAttribute("Id")
    $workingDirectory = Get-WixAncestorContainerId $mainFile
    if ([string]::IsNullOrWhiteSpace($mainFileId) -or [string]::IsNullOrWhiteSpace($workingDirectory)) {
      return $false
    }

    $desktopDirectory = $targetDir.SelectSingleNode("wix:Directory[@Id='DesktopFolder']", $ns)
    if (-not $desktopDirectory) {
      $desktopDirectory = New-WixElement $doc "Directory"
      $null = Set-WixAttribute $desktopDirectory "Id" "DesktopFolder"
      $null = Set-WixAttribute $desktopDirectory "Name" "Desktop"
      $null = $targetDir.AppendChild($desktopDirectory)
      $changed = $true
    }

    $desktopDirectoryRef = $product.SelectSingleNode("wix:DirectoryRef[@Id='DesktopFolder']", $ns)
    if (-not $desktopDirectoryRef) {
      $desktopDirectoryRef = New-WixElement $doc "DirectoryRef"
      $null = Set-WixAttribute $desktopDirectoryRef "Id" "DesktopFolder"
      $null = $product.AppendChild($desktopDirectoryRef)
      $changed = $true
    }

    $desktopShortcutComponent = $desktopDirectoryRef.SelectSingleNode("wix:Component[@Id='ApplicationDesktopShortcut']", $ns)
    if (-not $desktopShortcutComponent) {
      $desktopShortcutComponent = New-WixElement $doc "Component"
      $null = Set-WixAttribute $desktopShortcutComponent "Id" "ApplicationDesktopShortcut"
      $null = Set-WixAttribute $desktopShortcutComponent "Guid" "*"
      $null = $desktopDirectoryRef.AppendChild($desktopShortcutComponent)
      $changed = $true
    } else {
      $changed = (Set-WixAttribute $desktopShortcutComponent "Guid" "*") -or $changed
    }

    $shortcut = $desktopShortcutComponent.SelectSingleNode("wix:Shortcut[@Id='ApplicationDesktopShortcut']", $ns)
    if (-not $shortcut) {
      $shortcut = New-WixElement $doc "Shortcut"
      $null = Set-WixAttribute $shortcut "Id" "ApplicationDesktopShortcut"
      $null = $desktopShortcutComponent.AppendChild($shortcut)
      $changed = $true
    }

    $changed = (Set-WixAttribute $shortcut "Name" "termua") -or $changed
    $changed = (Set-WixAttribute $shortcut "Description" "termua") -or $changed
    $changed = (Set-WixAttribute $shortcut "Target" ("[#" + $mainFileId + "]")) -or $changed
    $changed = (Set-WixAttribute $shortcut "WorkingDirectory" $workingDirectory) -or $changed

    $registryValue = $desktopShortcutComponent.SelectSingleNode("wix:RegistryValue[@Name='desktop-shortcut']", $ns)
    if (-not $registryValue) {
      $registryValue = New-WixElement $doc "RegistryValue"
      $null = Set-WixAttribute $registryValue "Root" "HKCU"
      $null = Set-WixAttribute $registryValue "Key" "Software\termua"
      $null = Set-WixAttribute $registryValue "Name" "desktop-shortcut"
      $null = Set-WixAttribute $registryValue "Type" "integer"
      $null = Set-WixAttribute $registryValue "Value" "1"
      $null = Set-WixAttribute $registryValue "KeyPath" "yes"
      $null = $desktopShortcutComponent.AppendChild($registryValue)
      $changed = $true
    } else {
      $changed = (Set-WixAttribute $registryValue "Root" "HKCU") -or $changed
      $changed = (Set-WixAttribute $registryValue "Key" "Software\termua") -or $changed
      $changed = (Set-WixAttribute $registryValue "Type" "integer") -or $changed
      $changed = (Set-WixAttribute $registryValue "Value" "1") -or $changed
      $changed = (Set-WixAttribute $registryValue "KeyPath" "yes") -or $changed
    }

    $binariesFeature = $product.SelectSingleNode(".//wix:Feature[@Id='Binaries']", $ns)
    if ($binariesFeature) {
      $changed = (Add-WixComponentRefToFeature $doc $ns $binariesFeature "ApplicationDesktopShortcut") -or $changed
    }

    return $changed
  }
}

function Ensure-WixRelayBinary([string] $repoRoot, [string] $target) {
  $relayExe = Join-Path $repoRoot "target\$target\release\termua-relay.exe"
  if (-not (Test-Path $relayExe)) {
    throw "missing relay binary after build: $relayExe"
  }

  Invoke-WxsFileUpdate $repoRoot {
    param(
      [System.Xml.XmlDocument] $doc,
      [System.Xml.XmlNamespaceManager] $ns,
      [string] $filePath
    )

    $changed = $false
    $product = $doc.SelectSingleNode("/wix:Wix/wix:Product", $ns)
    if (-not $product) { return $false }

    $mainFile = $doc.SelectSingleNode("//wix:File[@Name='termua.exe']", $ns)
    if (-not $mainFile) { return $false }

    $mainComponent = $mainFile.ParentNode
    if (-not $mainComponent -or $mainComponent.LocalName -ne "Component") { return $false }

    $mainDirectory = $mainComponent.ParentNode
    if (-not $mainDirectory -or ($mainDirectory.LocalName -ne "Directory" -and $mainDirectory.LocalName -ne "DirectoryRef")) {
      return $false
    }

    $relayComponent = $mainDirectory.SelectSingleNode("wix:Component[@Id='RelayExecutable']", $ns)
    if (-not $relayComponent) {
      $relayComponent = New-WixElement $doc "Component"
      $null = Set-WixAttribute $relayComponent "Id" "RelayExecutable"
      $null = Set-WixAttribute $relayComponent "Guid" "*"
      $relayFile = New-WixElement $doc "File"
      $null = Set-WixAttribute $relayFile "Id" "termuaRelayExeFile"
      $null = Set-WixAttribute $relayFile "Name" "termua-relay.exe"
      $null = Set-WixAttribute $relayFile "Source" '$(var.CargoTargetBinDir)\termua-relay.exe'
      $null = Set-WixAttribute $relayFile "KeyPath" "yes"
      $null = Set-WixAttribute $relayFile "Checksum" "yes"
      $null = $relayComponent.AppendChild($relayFile)
      $null = $mainDirectory.AppendChild($relayComponent)
      $changed = $true
    } else {
      $changed = (Set-WixAttribute $relayComponent "Guid" "*") -or $changed
      $relayFile = $relayComponent.SelectSingleNode("wix:File[@Name='termua-relay.exe']", $ns)
      if (-not $relayFile) {
        $relayFile = New-WixElement $doc "File"
        $null = $relayComponent.AppendChild($relayFile)
        $changed = $true
      }
      $changed = (Set-WixAttribute $relayFile "Id" "termuaRelayExeFile") -or $changed
      $changed = (Set-WixAttribute $relayFile "Name" "termua-relay.exe") -or $changed
      $changed = (Set-WixAttribute $relayFile "Source" '$(var.CargoTargetBinDir)\termua-relay.exe') -or $changed
      $changed = (Set-WixAttribute $relayFile "KeyPath" "yes") -or $changed
      $changed = (Set-WixAttribute $relayFile "Checksum" "yes") -or $changed
    }

    $binariesFeature = $product.SelectSingleNode(".//wix:Feature[@Id='Binaries']", $ns)
    if ($binariesFeature) {
      $changed = (Add-WixComponentRefToFeature $doc $ns $binariesFeature "RelayExecutable") -or $changed
    }

    return $changed
  }
}

function Invoke-CargoWixPackage([string] $target) {
  $args = @("wix", "--package", "termua", "--no-build", "--target", $target, "--nocapture")
  $output = & cargo @args 2>&1
  $exitCode = $LASTEXITCODE
  $output | ForEach-Object { $_ }

  if ($exitCode -eq 0) {
    return
  }

  $outputText = ($output | Out-String)
  $windowsInstallerUnavailable =
    $outputText -match 'LGHT0217' -or
    $outputText -match 'Windows Installer Service could not be accessed'

  if ($windowsInstallerUnavailable) {
    Write-Warning "WiX validation could not access Windows Installer. Retrying with MSI validation suppressed (-sval)."
    & cargo wix --package termua --no-build --target $target --nocapture -L -sval
    if ($LASTEXITCODE -eq 0) {
      return
    }
  }

  throw "cargo wix failed ($exitCode)"
}

if ($env:OS -notlike "*Windows*") {
  Write-Error "This script is intended to run on Windows."
}

$arch = $env:ARCH
if ([string]::IsNullOrWhiteSpace($arch)) {
  try {
    $osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($osArch -eq [System.Runtime.InteropServices.Architecture]::X64) {
      $arch = "x86_64"
    } elseif ($osArch -eq [System.Runtime.InteropServices.Architecture]::Arm64) {
      $arch = "aarch64"
    }
  } catch {
    # ignore
  }
}
if ([string]::IsNullOrWhiteSpace($arch)) {
  # Best-effort fallback
  if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    $arch = "aarch64"
  } else {
    $arch = "x86_64"
  }
}

$target = $env:TARGET
if ([string]::IsNullOrWhiteSpace($target)) {
  if ($arch -eq "x86_64") {
    $target = "x86_64-pc-windows-msvc"
  } elseif ($arch -eq "aarch64") {
    $target = "aarch64-pc-windows-msvc"
  } else {
    Write-Error "Unsupported ARCH=$arch (expected x86_64 or aarch64)."
  }
}

$repoRoot = RepoRoot
Set-Location $repoRoot

Ensure-Tool "cargo" "Install Rust from https://rustup.rs/"
Ensure-CargoWix

Try-InstallWixToolset
if (-not (Ensure-WixToolsetAvailable)) {
  $found = Find-WixToolsetBin
  if ($found) {
    Write-Error "WiX Toolset bin exists but isn't in PATH: $found. Add it to PATH and re-run. (Need WiX v3.x: candle.exe + light.exe)"
  } else {
    Write-Error @"
WiX Toolset not found (need candle.exe and light.exe; WiX v3.x).

Download WiX v3 from:
  https://github.com/wixtoolset/wix3/releases

After install/extract, add its bin directory to PATH and re-run.
"@
  }
}

$outDir = $env:OUT_DIR
if ([string]::IsNullOrWhiteSpace($outDir)) {
  $outDir = "target\\msi\\$arch"
}

Write-Host "==> Building termua + termua-relay (release)"
& cargo build -p termua --release --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
& cargo build -p termua_relay --release --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo build termua_relay failed ($LASTEXITCODE)" }

if ((Find-WxsFiles $repoRoot).Count -eq 0) {
  Write-Host "==> Initializing WiX sources (cargo wix init)"
  & cargo wix init --package termua
  if ($LASTEXITCODE -ne 0) { throw "cargo wix init failed ($LASTEXITCODE)" }

  if ((Find-WxsFiles $repoRoot).Count -eq 0) {
    Write-Error "cargo wix init completed but no .wxs files were found under wix/ or termua/wix/"
  }
}

$null = Ensure-WixDesktopShortcut $repoRoot
$icoPath = Ensure-TermuaIco $repoRoot $arch
if ($icoPath) {
  Ensure-WixIcon $repoRoot $icoPath
}
Ensure-WixRelayBinary $repoRoot $target

Write-Host "==> Packaging MSI (cargo wix)"
Invoke-CargoWixPackage $target

$msiPath = Find-LatestMsi $repoRoot
if (-not $msiPath) {
  Write-Error "Failed to locate generated .msi under target\\wix"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$packageVersion = Get-CargoPackageVersion "termua"
$destName = "termua-$packageVersion-windows.$arch.msi"
$dest = Join-Path $outDir $destName
Copy-Item -Force $msiPath $dest

Write-Host "==> Wrote: $dest"

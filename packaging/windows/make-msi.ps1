$ErrorActionPreference = "Stop"

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

function Get-WixCompatibleVersion([string] $version) {
  if ([string]::IsNullOrWhiteSpace($version)) {
    throw "Package version is empty."
  }

  $match = [regex]::Match(
    $version,
    '^(?<core>\d+\.\d+\.\d+)(?:-(?<pre>[0-9A-Za-z.-]+))?(?:\+(?<build>[0-9A-Za-z.-]+))?$'
  )
  if (-not $match.Success) {
    throw "Unsupported package version format for WiX packaging: '$version'"
  }

  $pre = $match.Groups['pre'].Value
  if ([string]::IsNullOrWhiteSpace($pre)) {
    return $version
  }

  $identifiers = $pre -split '\.'
  $lastIdentifier = $identifiers[$identifiers.Length - 1]
  if ($lastIdentifier -match '^\d+$') {
    return $version
  }

  $build = $match.Groups['build'].Value
  $buildSuffix = ""
  if (-not [string]::IsNullOrWhiteSpace($build)) {
    $buildSuffix = "+$build"
  }

  return "$($match.Groups['core'].Value)-$pre.0$buildSuffix"
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

function Find-LatestMsi([string] $repoRoot, [string] $target) {
  $candidates = @(
    Join-Path $repoRoot "target\\wix"
    (Join-Path $repoRoot ("target\\{0}\\wix" -f $target))
    Join-Path $repoRoot "termua\\target\\wix"
    (Join-Path $repoRoot ("termua\\target\\{0}\\wix" -f $target))
  ) | Select-Object -Unique

  $existingCandidates = @($candidates | Where-Object { Test-Path $_ })
  foreach ($dir in $existingCandidates) {
    $msi = Get-ChildItem -Path $dir -Recurse -Filter "*.msi" -ErrorAction SilentlyContinue |
      Sort-Object -Property LastWriteTime -Descending |
      Select-Object -First 1
    if ($msi) {
      return @{
        Path = $msi.FullName
        Searched = $candidates
      }
    }
  }

  # cargo-wix may emit into an unexpected target subdirectory when --target is set.
  $fallbackRoots = @(
    Join-Path $repoRoot "target"
    Join-Path $repoRoot "termua\\target"
  ) | Select-Object -Unique
  foreach ($root in $fallbackRoots) {
    if (-not (Test-Path $root)) { continue }

    $msi = Get-ChildItem -Path $root -Recurse -Filter "*.msi" -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -match '[\\/]wix[\\/]' } |
      Sort-Object -Property LastWriteTime -Descending |
      Select-Object -First 1
    if ($msi) {
      return @{
        Path = $msi.FullName
        Searched = $candidates
      }
    }
  }

  return @{
    Path = $null
    Searched = $candidates
  }
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

  foreach ($file in $wxsFiles) {
    $content = Get-Content -Raw -Path $file.FullName
    $original = $content

    $iconPattern = "<Icon\b(?=[^>]*\bId\s*=\s*[""'']termuaIcon[""''])[^>]*/>\s*"
    $arpPattern = "<Property\b(?=[^>]*\bId\s*=\s*[""'']ARPPRODUCTICON[""''])[^>]*/>\s*"
    $metadata = (
      '    <Icon Id="termuaIcon" SourceFile="' + $iconSource + '" />' + "`r`n" +
      '    <Property Id="ARPPRODUCTICON" Value="termuaIcon" />' + "`r`n"
    )

    $content = [regex]::Replace(
      $content,
      $iconPattern,
      "",
      [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )
    $content = [regex]::Replace(
      $content,
      $arpPattern,
      "",
      [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )

    $insertionReplacement = '$1' + "`r`n" + $metadata
    if ($content -match '<Package\b[^>]*/>') {
      $content = [regex]::Replace(
        $content,
        '(<Package\b[^>]*/>)',
        $insertionReplacement,
        [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
      )
    } else {
      $content = [regex]::Replace(
        $content,
        '(<Product\b[^>]*>)',
        $insertionReplacement,
        [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
      )
    }

    # Try to set shortcut icons if the template contains shortcuts.
    $content = [regex]::Replace(
      $content,
      "<Shortcut\\b(?![^>]*\\bIcon=)([^>]*)>",
      '<Shortcut Icon="termuaIcon"$1>',
      [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )

    if ($content -ne $original) {
      $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
      [System.IO.File]::WriteAllText($file.FullName, $content, $utf8NoBom)
    }
  }
}

function Ensure-WixRelayBinary([string] $repoRoot, [string] $target) {
  $relayExe = Join-Path $repoRoot "target\$target\release\termua-relay.exe"
  if (-not (Test-Path $relayExe)) {
    throw "missing relay binary after build: $relayExe"
  }

  $wxsFiles = Find-WxsFiles $repoRoot
  if (-not $wxsFiles -or $wxsFiles.Count -eq 0) { return }

  foreach ($file in $wxsFiles) {
    $content = Get-Content -Raw -Path $file.FullName
    $original = $content

    $regexOptions = [System.Text.RegularExpressions.RegexOptions]::IgnoreCase -bor [System.Text.RegularExpressions.RegexOptions]::Singleline
    $relayFilePattern = "<File\b[^>]*Name\s*=\s*[""'']termua-relay\.exe[""''][^>]*/>\s*"
    $relayComponentPattern = "\s*<Component\b[^>]*Id\s*=\s*[""'']RelayExecutable[""''][^>]*>.*?<File\b[^>]*Name\s*=\s*[""'']termua-relay\.exe[""''][^>]*/>\s*</Component>\s*"
    $relayComponentRefPattern = "<ComponentRef\s+Id\s*=\s*[""'']RelayExecutable[""'']\s*/>\s*"

    $content = [regex]::Replace($content, $relayComponentPattern, "", $regexOptions)
    $content = [regex]::Replace($content, $relayFilePattern, "", $regexOptions)
    $content = [regex]::Replace($content, $relayComponentRefPattern, "", [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)

    $relayComponent =
      '          <Component Id="RelayExecutable" Guid="*">' + "`r`n" +
      '            <File Id="termuaRelayExeFile" Name="termua-relay.exe" Source="$(var.CargoTargetBinDir)\termua-relay.exe" KeyPath="yes" Checksum="yes" />' + "`r`n" +
      '          </Component>'

    $mainComponentMatch = [regex]::Match(
      $content,
      '<Component\b[^>]*\bId\s*=\s*["' + "'" + '](?<id>[^"' + "'" + ']+)["' + "'" + '][^>]*>(?:(?!<Component\b).)*?<File\b[^>]*\bName\s*=\s*["' + "'" + ']termua\.exe["' + "'" + '][^>]*/>(?:(?!<Component\b).)*?</Component>',
      $regexOptions
    )
    if (-not $mainComponentMatch.Success) {
      continue
    }

    $mainComponentId = $mainComponentMatch.Groups['id'].Value
    $insertAt = $mainComponentMatch.Index + $mainComponentMatch.Length
    $content = $content.Insert($insertAt, "`r`n" + $relayComponent)

    $mainComponentRefMatch = [regex]::Match(
      $content,
      '<ComponentRef\b[^>]*\bId\s*=\s*["' + "'" + ']' + [regex]::Escape($mainComponentId) + '["' + "'" + '][^>]*/>',
      [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )
    if ($mainComponentRefMatch.Success) {
      $refInsertAt = $mainComponentRefMatch.Index + $mainComponentRefMatch.Length
      $content = $content.Insert($refInsertAt, "`r`n" + '            <ComponentRef Id="RelayExecutable" />')
    } elseif ($content -match '</Feature>') {
      $content = [regex]::Replace(
        $content,
        '(</Feature>)',
        '            <ComponentRef Id="RelayExecutable" />' + "`r`n" + '$1',
        [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
      )
    }

    if ($content -ne $original) {
      $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
      [System.IO.File]::WriteAllText($file.FullName, $content, $utf8NoBom)
    }
  }
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

$icoPath = Ensure-TermuaIco $repoRoot $arch
if ($icoPath) {
  Ensure-WixIcon $repoRoot $icoPath
}
Ensure-WixRelayBinary $repoRoot $target

$packageVersion = Get-CargoPackageVersion "termua"
$wixVersion = Get-WixCompatibleVersion $packageVersion
if ($wixVersion -ne $packageVersion) {
  Write-Host "==> Using WiX-compatible version $wixVersion (from package version $packageVersion)"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$destName = "termua-$packageVersion-windows.$arch.msi"
$dest = Join-Path $outDir $destName

Write-Host "==> Packaging MSI (cargo wix)"
& cargo wix --package termua --version $wixVersion --output $dest --no-build --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo wix failed ($LASTEXITCODE)" }

if (-not (Test-Path $dest)) {
  $msiResult = Find-LatestMsi $repoRoot $target
  $msiPath = $msiResult.Path
  if (-not $msiPath) {
    $searched = ($msiResult.Searched | ForEach-Object { "  - $_" }) -join "`r`n"
    Write-Error "Failed to locate generated .msi. Checked:`r`n$searched"
  }

  Copy-Item -Force $msiPath $dest
}

Write-Host "==> Wrote: $dest"

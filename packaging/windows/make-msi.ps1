$ErrorActionPreference = "Stop"

<#
.SYNOPSIS
  Build an MSI for termua on Windows using cargo-wix.

.REQUIRES
  - Rust toolchain (cargo)
  - WiX Toolset (candle.exe + light.exe in PATH)

.USAGE
  powershell -ExecutionPolicy Bypass -File packaging/windows/make-msi.ps1

  # Custom output directory
  $env:OUT_DIR="target\\msi"
  powershell -ExecutionPolicy Bypass -File packaging/windows/make-msi.ps1

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

  foreach ($file in $wxsFiles) {
    $content = Get-Content -Raw -Path $file.FullName
    $original = $content

    $hasIcon = $content -match '<Icon\s+Id="termuaIcon"\b'
    $hasArp = $content -match '<Property\s+Id="ARPPRODUCTICON"\b'

    if (-not $hasIcon -or -not $hasArp) {
      $insertion = ""
      if (-not $hasIcon) {
        $insertion += "    <Icon Id=`"termuaIcon`" SourceFile=`"termua.ico`" />`r`n"
      }
      if (-not $hasArp) {
        $insertion += "    <Property Id=`"ARPPRODUCTICON`" Value=`"termuaIcon`" />`r`n"
      }

      $content = [regex]::Replace(
        $content,
        "(<Product\\b[^>]*>\\s*)",
        "`$1`r`n$insertion",
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

    if ($content -match 'Name="termua-relay\.exe"' -or $content -match "Name='termua-relay\.exe'") {
      continue
    }

    $relayReplacement =
      '$1' +
      "`r`n" +
      '              <File Id="termuaRelayExeFile" Name="termua-relay.exe" DiskId="1" Source="$(var.CargoTargetBinDir)\termua-relay.exe" Checksum="yes" />'

    $content = [regex]::Replace(
      $content,
      '(<File\b[^>]*Name=(["' + "'" + '])termua\.exe\2[^>]*/>)',
      $relayReplacement,
      [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )

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

Write-Host "==> Packaging MSI (cargo wix)"
& cargo wix --package termua --no-build --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo wix failed ($LASTEXITCODE)" }

$msiPath = Find-LatestMsi $repoRoot
if (-not $msiPath) {
  Write-Error "Failed to locate generated .msi under target\\wix"
}

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$leaf = Split-Path -Leaf $msiPath
$destName = $leaf
if ($destName -notlike "*$arch*") {
  $base = [System.IO.Path]::GetFileNameWithoutExtension($leaf)
  $destName = "$base-$arch.msi"
}
$dest = Join-Path $outDir $destName
Copy-Item -Force $msiPath $dest

Write-Host "==> Wrote: $dest"

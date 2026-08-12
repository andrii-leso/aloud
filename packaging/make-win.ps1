# Assembles target\release into a folder that runs on a machine which is NOT a
# development box. The Windows counterpart of make-app.sh, and deliberately the
# same shape: no tauri-cli, no installer, just the release binary plus the files
# it genuinely needs beside it.
#
# WHY THIS EXISTS. `cargo build --release` alone produces an aloud.exe that hard-
# imports MSVCP140.dll, MSVCP140_1.dll, VCRUNTIME140.dll and VCRUNTIME140_1.dll.
# Those are the VC++ redistributable, NOT an OS component: they are present on
# this machine only because Visual Studio's "Desktop development with C++"
# workload installed them. On a clean Windows install the exe fails at load with
# no useful message. That is hard constraint 1 ("zero runtime system
# dependencies - no 'first install X' step, ever") being violated silently, and
# it is invisible from a dev box because a dev box always has the redist.
#
# Microsoft sanctions the fix verbatim, calling it local deployment: "library
# files are installed in your application folder together with the executable
# file." No user-visible install step, so constraint 1 holds in general and not
# merely on this machine.
#
# WHY NOT STATIC_VCRUNTIME. tauri-build reads a STATIC_VCRUNTIME=true env var and
# emits /NODEFAULTLIB:msvcrt.lib + /DEFAULTLIB:libcmt.lib, which looks like it
# would remove the problem with no shipped files at all. Measured on 2026-08-11:
# it LINKS against ort, and it removes VCRUNTIME140.dll and VCRUNTIME140_1.dll -
# but MSVCP140.dll and MSVCP140_1.dll REMAIN, because those are the C++ standard
# library that ONNX Runtime's own C++ pulls in and static_vcruntime does not
# cover it. So it fixes half the problem, and in exchange it creates a worse one:
# a statically linked ucrt inside aloud.exe while msvcp140.dll drags in
# vcruntime140.dll and ucrtbase.dll behind it. Two CRT instances means two heaps,
# and an allocation crossing between them is corruption that will not reproduce
# on demand. One consistent dynamic CRT with the DLLs shipped alongside is the
# boring, correct answer. Do not "improve" this back to STATIC_VCRUNTIME.
#
# NOT COVERED HERE, on purpose:
#   - The 385 MB Supertonic model. Not shipped; resolved at runtime (see
#     src/lib.rs model_dir()) and there is still no first-run download.
#   - WebView2. Assumed present; it ships with Windows 11.
#   - Code signing. None. Unsigned is fine for a local prototype - Windows has
#     no TCC analogue, and SmartScreen does not fire on a local build output.
#   - An installer. Deliberately not built; a portable folder is the whole point.

$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Release = Join-Path $Root 'target\release'
$Exe = Join-Path $Release 'aloud.exe'

# The four the exe actually imports. Kept as an explicit list rather than a
# wildcard copy of the whole redist folder: shipping DLLs nothing links against
# is how a "portable" folder quietly grows.
$CrtDlls = @('MSVCP140.dll', 'MSVCP140_1.dll', 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll')

Write-Host '==> building release binary (aloud)'
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
# STATIC_VCRUNTIME must be absent - see the header. If the caller's shell has it
# set, the build would silently produce the mixed-CRT binary this script exists
# to avoid.
if ($env:STATIC_VCRUNTIME) {
    Write-Host '    STATIC_VCRUNTIME was set in this shell; clearing it for the build'
    Remove-Item Env:\STATIC_VCRUNTIME
}
Push-Location $Root
try {
    cargo build --release --bin aloud
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
}
finally { Pop-Location }

if (-not (Test-Path $Exe)) { throw "no binary at $Exe" }

Write-Host '==> locating the VC++ redistributable CRT'
# Highest-numbered versioned folder. The sibling `v143` is an alias directory and
# does not contain the DLLs, so it is filtered out by requiring a numeric name.
$redistRoot = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools\VC\Redist\MSVC'
if (-not (Test-Path $redistRoot)) {
    $redistRoot = Join-Path $env:ProgramFiles 'Microsoft Visual Studio\2022\BuildTools\VC\Redist\MSVC'
}
if (-not (Test-Path $redistRoot)) {
    throw "no VC redist root found. Install the 'Desktop development with C++' workload, or set the path by hand."
}
$version = Get-ChildItem $redistRoot -Directory |
    Where-Object { $_.Name -match '^\d+\.' } |
    Sort-Object { [version]($_.Name) } -Descending |
    Select-Object -First 1
if (-not $version) { throw "no versioned CRT folder under $redistRoot" }
$crtDir = Join-Path $version.FullName 'x64\Microsoft.VC143.CRT'
if (-not (Test-Path $crtDir)) { throw "expected the CRT at $crtDir" }
Write-Host "    $crtDir"

Write-Host '==> copying the CRT beside the exe'
foreach ($dll in $CrtDlls) {
    $src = Join-Path $crtDir $dll
    if (-not (Test-Path $src)) { throw "missing $dll in $crtDir" }
    Copy-Item $src (Join-Path $Release $dll) -Force
    Write-Host ("    {0,-22} {1,6:N0} KB" -f $dll, ((Get-Item $src).Length / 1KB))
}

# The check that makes this script worth running. dumpbin is the only thing that
# can say what the binary ACTUALLY imports; the list above is a claim until it is
# compared against reality. A new dependency appearing in a future build (another
# crate pulling in a C++ library, say) shows up here rather than as a load
# failure on someone else's machine.
Write-Host '==> verifying imports'
$dumpbin = Get-ChildItem (Join-Path $version.FullName '..\..\..\Tools\MSVC') -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match 'Hostx64\\x64' } | Select-Object -First 1
if (-not $dumpbin) {
    Write-Warning 'dumpbin not found - skipping the import check. Run this from a Developer PowerShell to enable it.'
}
else {
    $imports = & $dumpbin.FullName /dependents $Exe |
        Select-String '^\s+\S+\.dll' | ForEach-Object { $_.ToString().Trim() }
    # api-ms-win-* are the OS API sets and ship with Windows; everything else
    # named here must either be an OS DLL or sit in this folder.
    $unshipped = $imports | Where-Object {
        $_ -notmatch '^api-ms-win-' -and
        $_ -match '^(MSVCP|VCRUNTIME|CONCRT|onnxruntime)'
    } | Where-Object { -not (Test-Path (Join-Path $Release $_)) }

    if ($unshipped) {
        throw "these are imported but not shipped beside the exe: $($unshipped -join ', ')"
    }
    Write-Host '    every non-OS import is satisfied in the output folder'
}

Write-Host ''
Write-Host "==> done: $Release"
Get-ChildItem $Release -File | Where-Object { $_.Extension -in '.exe', '.dll' } |
    Sort-Object Length -Descending |
    Format-Table Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 2) } } -AutoSize
Write-Host 'Copy this folder to another machine to test it. It still needs the model:'
Write-Host '  set ALOUD_MODEL_DIR, or let model_dir() fall back to %LOCALAPPDATA%\com.andriileso.aloud\supertonic3'

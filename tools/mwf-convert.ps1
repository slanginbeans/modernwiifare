# Build and run the converter (mwf-convert) on Windows with a toolchain that works:
#   powershell -ExecutionPolicy Bypass -File tools\mwf-convert.ps1 <converter arguments...>
#
# The plain Windows GNU toolchain can't build it: IW4L's zone reader uses the `windows` crate, whose import
# libraries need a dlltool + assembler the toolchain doesn't ship. The converter is built for
# x86_64-pc-windows-gnullvm instead, linked by llvm-mingw:
#   1. MWF_RUST_ENV, or rust\env.ps1 at the root of the repo's drive (e.g. D:\rust\env.ps1), when present, sets that
#      up (see docs/runtime.md);
#   2. otherwise this script does: llvm-mingw (pinned release, checked by SHA-256) into .toolchain\llvm-mingw, and the
#      gnullvm target added to your rustup.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$MingwRelease = '20261104'
$MingwZip = "llvm-mingw-$MingwRelease-ucrt-x86_64.zip"
$MingwSha256 = '552191B5754C49A5A20723276A6FC56541EA1922C4C50B7B669022333FC1B40F'

$envScript = if ($env:MWF_RUST_ENV) { $env:MWF_RUST_ENV } else { Join-Path ([IO.Path]::GetPathRoot($repo)) 'rust\env.ps1' }
if (Test-Path $envScript) {
    . $envScript
} else {
    $tc = Join-Path $repo '.toolchain'
    $mingw = Join-Path $tc 'llvm-mingw'
    if (-not (Test-Path (Join-Path $mingw 'bin\clang.exe'))) {
        Write-Host "[rust] The converter needs llvm-mingw (a C toolchain, about 190 MB download, 900 MB unpacked)"
        Write-Host "       in $mingw. Nothing is installed system-wide."
        $answer = Read-Host '[rust] Download it now? (y/n)'
        if ($answer -notmatch '^[yY]') { Write-Host '[rust] Not downloading.'; exit 1 }
        New-Item -ItemType Directory -Force $tc | Out-Null
        $zip = Join-Path $tc $MingwZip
        & curl.exe -sSfL -o $zip "https://github.com/mstorsjo/llvm-mingw/releases/download/$MingwRelease/$MingwZip"
        if ($LASTEXITCODE -ne 0) { Write-Host '[rust] Download failed.'; exit 1 }
        if ((Get-FileHash $zip -Algorithm SHA256).Hash -ne $MingwSha256) {
            Remove-Item $zip
            Write-Host '[rust] The download does not match the pinned checksum. Not using it.'
            exit 1
        }
        Write-Host '[rust] Unpacking...'
        Expand-Archive -Path $zip -DestinationPath $tc -Force
        Rename-Item (Join-Path $tc ($MingwZip -replace '\.zip$', '')) 'llvm-mingw'
        Remove-Item $zip
    }
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    $env:PATH = "$cargoBin;$mingw\bin;$env:PATH"
    & rustup target add x86_64-pc-windows-gnullvm | Out-Null
    $sysroot = (& rustc --print sysroot).Trim()
    $env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-gnullvm'
    # Build scripts still run on the GNU host: keep its own bundled linker.
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained\x86_64-w64-mingw32-gcc.exe'
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER = Join-Path $mingw 'bin\x86_64-w64-mingw32-clang.exe'
    $env:CC_x86_64_pc_windows_gnullvm = Join-Path $mingw 'bin\x86_64-w64-mingw32-clang.exe'
    $env:AR_x86_64_pc_windows_gnullvm = Join-Path $mingw 'bin\llvm-ar.exe'
}

Write-Host '[convert] Building the converter (the first time takes a few minutes)...'
& cargo build --release -q -p mwf_convert
if ($LASTEXITCODE -ne 0) { exit 1 }
& (Join-Path $repo 'target\x86_64-pc-windows-gnullvm\release\mwf-convert.exe') @args
exit $LASTEXITCODE

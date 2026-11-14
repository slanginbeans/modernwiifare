@echo off
rem ---------------------------------------------------------------------------
rem  Modern Wiifare - Windows build
rem
rem  Double-click this file (or run it from a command prompt). It:
rem    1. updates this folder from the GitHub repo (git pull), if it can
rem    2. builds the Wii programs (Rust + devkitPro)
rem    3. packages them for the Homebrew Channel:
rem         dist\apps\mwf_game\boot.dol + meta.xml (and the test spikes)
rem    4. if it finds your MW2 / IW4x folder, converts the maps and weapons that
rem       are missing or out of date (only those: an up-to-date pack is skipped)
rem       and copies them into dist\apps\mwf_game (maps\, views\, common.wpk)
rem       Copy the "apps" folder inside "dist" to the root of the SD card.
rem
rem  Needs:
rem    - devkitPro with the Wii tools (installer: github.com/devkitPro/installer,
rem      tick "Wii Development"), plus Rust (this script offers to install it), or
rem    - Docker Desktop (it downloads the devkitPro image and Rust the first time)
rem  and Git for Windows for the automatic update (optional).
rem
rem  Step 4 reads your own game files (never put in this repo). Which folder, in order:
rem    - MWF_GAME=D:\path\to\game (set before running)
rem    - "Call of Duty Modern Warfare 2" next to this repo folder
rem      (e.g. D:\mwf\Call of Duty Modern Warfare 2 beside D:\mwf\modernwiifare)
rem  Without one, the build still works: copy converted packs yourself.
rem
rem    build.bat           update, then build, package and convert what changed
rem    build.bat noupdate  the same without updating
rem    build.bat nodata    build and package only (no game data step)
rem    build.bat clean     delete build output
rem ---------------------------------------------------------------------------
setlocal
cd /d "%~dp0"

set "TARGET=package"
set "UPDATE=1"
if /i "%~1"=="clean" set "TARGET=clean"
if /i "%~1"=="clean" set "UPDATE=0"
if /i "%~1"=="noupdate" set "UPDATE=0"
set "DATA=1"
if /i "%~1"=="nodata" set "DATA=0"

rem ---- 1. update from the repo ----------------------------------------------
if "%UPDATE%"=="0" goto build
rem Prefer Git for Windows: devkitPro's installer can put its own git earlier
rem on the PATH.
set "GIT=git"
if exist "%ProgramFiles%\Git\cmd\git.exe" (
    set "GIT=%ProgramFiles%\Git\cmd\git.exe"
) else (
    where git >nul 2>nul
    if errorlevel 1 (
        echo [update] Git isn't installed, so not updating. Get it from git-scm.com.
        goto build
    )
)
if not exist ".git" (
    echo [update] This folder isn't a git clone ^(downloaded as a ZIP?^), so not updating.
    echo          To get updates automatically: git clone https://github.com/slanginbeans/modernwiifare
    goto build
)
echo [update] Getting the latest version...
set "GIT_TERMINAL_PROMPT=0"
"%GIT%" pull --ff-only
if errorlevel 1 (
    echo.
    echo [update] Couldn't update. Building what's here instead. Common reasons:
    echo   - No internet.
    echo   - Files in this folder were changed. To throw those changes away:
    echo       "%GIT%" reset --hard origin/main
    echo.
)

:build
rem ---- 2/3. build and package ------------------------------------------------
rem devkitPro's Windows installer puts its tools in X:\devkitPro\msys2 (on
rem any drive); its make has to run inside that shell, so it's usually not on
rem the normal PATH. MWF_DEVKITPRO=D:\some\folder overrides the search.
set "DKP="
if defined MWF_DEVKITPRO if exist "%MWF_DEVKITPRO%\msys2\usr\bin\bash.exe" set "DKP=%MWF_DEVKITPRO%"
if not defined DKP for %%D in (C D E F G H I J K L M N O P Q R S T U V W X Y Z) do (
    if not defined DKP if exist "%%D:\devkitPro\msys2\usr\bin\bash.exe" set "DKP=%%D:\devkitPro"
)
rem the helper script, with forward slashes for devkitPro's shell
set "WINBUILD=%~dp0tools\winbuild.sh"
set "WINBUILD=%WINBUILD:\=/%"
if defined DKP (
    if not exist "%DKP%\devkitPPC\bin" (
        echo [build] devkitPro is installed but without the Wii tools.
        echo         Run the devkitPro installer again and tick "Wii Development".
        goto failed
    )
    if /i not "%TARGET%"=="clean" call :ensure_rust
    if errorlevel 1 goto failed
    echo [build] Using devkitPro in %DKP%: make %TARGET%
    "%DKP%\msys2\usr\bin\bash.exe" -l "%WINBUILD%" %TARGET%
    if errorlevel 1 goto failed
    goto done
)
rem devkitPro set up some other way, with make on the PATH
if defined DEVKITPPC (
    where make >nul 2>nul
    if not errorlevel 1 (
        if /i not "%TARGET%"=="clean" call :ensure_rust
        if errorlevel 1 goto failed
        echo [build] Using devkitPro: make -C wii %TARGET%
        make -C wii %TARGET%
        if errorlevel 1 goto failed
        goto done
    )
)

rem Docker Desktop (on the PATH, or where it installs)
set "DOCKER=docker"
where docker >nul 2>nul
if errorlevel 1 (
    if exist "%ProgramFiles%\Docker\Docker\resources\bin\docker.exe" (
        set "DOCKER=%ProgramFiles%\Docker\Docker\resources\bin\docker.exe"
    ) else (
        goto notools
    )
)
"%DOCKER%" info >nul 2>nul
if errorlevel 1 (
    echo [build] Docker Desktop is installed but not running. Start it, wait until it says
    echo         it's running, then run build.bat again.
    goto failed
)
echo [build] Using Docker ^(devkitpro/devkitppc^): make %TARGET%
echo         The first run also downloads Rust into .toolchain\ ^(a few minutes^).
"%DOCKER%" run --rm -v "%cd%":/src -w /src devkitpro/devkitppc sh tools/docker-build.sh %TARGET%
if errorlevel 1 goto failed
goto done

rem ---- Rust: needed to build the game code -----------------------------------
:ensure_rust
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" exit /b 0
where cargo >nul 2>nul
if not errorlevel 1 exit /b 0
echo.
echo [rust] Rust isn't installed. It's needed to build the game code.
echo        It installs into your user folder ^(%USERPROFILE%\.cargo^), no admin needed.
choice /c YN /m "[rust] Install Rust now"
if errorlevel 2 (
    echo [rust] Not installing. Get it later from https://rustup.rs and run build.bat again.
    exit /b 1
)
rem The GNU host toolchain brings its own linker, so Visual Studio isn't needed.
set "RUSTUP_INIT=%TEMP%\rustup-init.exe"
echo [rust] Downloading the Rust installer...
curl -sSfL -o "%RUSTUP_INIT%" https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe
if errorlevel 1 (
    echo [rust] Download failed. Install it by hand from https://rustup.rs
    exit /b 1
)
"%RUSTUP_INIT%" -y --default-host x86_64-pc-windows-gnu --profile minimal --default-toolchain none
if errorlevel 1 (
    echo [rust] The installer failed. Install it by hand from https://rustup.rs
    exit /b 1
)
del "%RUSTUP_INIT%" >nul 2>nul
echo [rust] Installed. The first build downloads the exact Rust version this project pins.
exit /b 0

rem ---- 4. game data: convert what changed, copy it into dist -----------------
:data
set "GAME=%MWF_GAME%"
rem (paths from the repo folder: the script cd-ed there at the start)
if not defined GAME if exist "%cd%\..\Call of Duty Modern Warfare 2\zone\" set "GAME=%cd%\..\Call of Duty Modern Warfare 2"
if not defined GAME (
    echo.
    echo [data] No game folder found, so no maps converted. Put this folder next to your
    echo        "Call of Duty Modern Warfare 2" folder, or set MWF_GAME to it, and build again.
    exit /b 0
)
set "GAME=%GAME:"=%"
call :ensure_rust
if errorlevel 1 exit /b 0
set "CARGO=cargo"
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
echo.
echo [data] Converting what changed from: %GAME%
echo        ^(everything the first time: about 10 minutes; up-to-date packs are skipped^)
"%CARGO%" run --release -q -p mwf_convert -- pack "%GAME%" --map all
if errorlevel 1 echo [data] Some conversions failed ^(see above^). Copying what was made.
if not exist "out\packs\" exit /b 0
set "APP=dist\apps\mwf_game"
rem robocopy copies only new or changed files; exit codes under 8 mean success.
robocopy "out\packs" "%APP%\maps" *.wpk /xf common.wpk /njh /njs /ndl /nfl /np >nul
if errorlevel 8 echo [data] Copying the maps failed.
if exist "out\packs\common.wpk" robocopy "out\packs" "%APP%" common.wpk /njh /njs /ndl /nfl /np >nul
if errorlevel 8 echo [data] Copying common.wpk failed.
if exist "out\packs\views\" robocopy "out\packs\views" "%APP%\views" *.wpk /njh /njs /ndl /nfl /np >nul
if errorlevel 8 echo [data] Copying the weapon files failed.
echo [data] Game data is in %APP% ^(maps\, views\, common.wpk^).
exit /b 0

:notools
echo.
echo [build] No Wii build tools found on this PC. The easiest fix:
echo.
echo   1. Download the devkitPro installer ^(devkitProUpdater-*.exe^) from
echo        https://github.com/devkitPro/installer/releases/latest
echo   2. Run it. Install it to a folder named devkitPro at the top of any drive
echo      ^(C:\devkitPro, D:\devkitPro, ...^). When it asks what to install,
echo      tick "Wii Development". Untick the rest.
echo   3. Run build.bat again. It offers to install Rust too, if you don't have it.
echo.
echo   ^(Docker Desktop works too, if you already use it.^)
echo.
echo   No installs at all: on github.com open the repo, Actions, the newest
echo   "ci" run, and download "modernwiifare-sd-card" at the bottom. It's the
echo   same dist\apps folder, built by GitHub.
goto failed

:done
if /i "%TARGET%"=="package" if "%DATA%"=="1" call :data
echo.
if /i "%TARGET%"=="package" (
    echo Done. Copy the "apps" folder from:
    echo   %cd%\dist
    echo to the root of the Wii's SD card, then launch from the Homebrew Channel.
) else (
    echo Cleaned.
)
echo.
if "%~1"=="" pause
endlocal
exit /b 0

:failed
echo.
echo Build failed. See the messages above.
echo.
if "%~1"=="" pause
endlocal
exit /b 1

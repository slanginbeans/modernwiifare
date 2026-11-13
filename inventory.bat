@echo off
rem ---------------------------------------------------------------------------
rem  Modern Wiifare - game data inventory (Phase 0, Spike D)
rem
rem  Double-click this file, or drag your game folder onto it. It:
rem    1. updates this folder from the GitHub repo (git pull), if it can
rem    2. builds the converter (Rust) the first time
rem    3. reads your MW2 / IW4x install and lists every zone with its sizes and
rem       how many assets of each type it holds
rem
rem  The report has only file names, sizes and counts: no game content, no
rem  absolute paths. It's saved to out\inventory.json (never committed).
rem
rem  Which game folder it reads, in order:
rem    - the folder you dragged onto this file, or the first argument
rem    - "Call of Duty Modern Warfare 2" next to this repo folder
rem      (e.g. D:\mwf\Call of Duty Modern Warfare 2 beside D:\mwf\modernwiifare)
rem    - otherwise it asks
rem
rem    inventory.bat                     update, build, run
rem    inventory.bat "D:\path\to\game"   same, for that folder
rem ---------------------------------------------------------------------------
setlocal
cd /d "%~dp0"

rem ---- 1. update from the repo ----------------------------------------------
set "GIT=git"
if exist "%ProgramFiles%\Git\cmd\git.exe" set "GIT=%ProgramFiles%\Git\cmd\git.exe"
if exist ".git" (
    echo [update] Getting the latest version...
    set "GIT_TERMINAL_PROMPT=0"
    "%GIT%" pull --ff-only >nul 2>nul
    if errorlevel 1 echo [update] Couldn't update ^(offline, or local changes^). Using what's here.
)

rem ---- 2. which game folder -------------------------------------------------
set "GAME=%~1"
if not defined GAME if exist "%~dp0..\Call of Duty Modern Warfare 2\" set "GAME=%~dp0..\Call of Duty Modern Warfare 2"
if not defined GAME (
    echo.
    echo Where is your MW2 / IW4x folder? ^(the one containing the "zone" folder^)
    set /p "GAME=Folder: "
)
if not defined GAME goto failed
rem strip quotes typed or pasted with the path
set "GAME=%GAME:"=%"
if not exist "%GAME%\" (
    echo [inventory] Not a folder: %GAME%
    goto failed
)

rem ---- 3. Rust ----------------------------------------------------------------
set "CARGO=cargo"
if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
    set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
) else (
    where cargo >nul 2>nul
    if errorlevel 1 (
        echo [rust] Rust isn't installed. Run build.bat once ^(it offers to install it^),
        echo        or get it from https://rustup.rs, then run this again.
        goto failed
    )
)

rem ---- 4. build + run -------------------------------------------------------
echo [inventory] Building the converter ^(the first time takes a few minutes^)...
"%CARGO%" build --release -q -p mwf_convert
if errorlevel 1 goto failed
echo [inventory] Reading: %GAME%
echo.
"%CARGO%" run --release -q -p mwf_convert -- inventory "%GAME%" --out out\inventory.json
if errorlevel 1 goto failed

echo.
echo Done. Send the text above ^(or out\inventory.json^) back to the project.
echo It contains only names, sizes and counts, no game files.
echo.
pause
endlocal
exit /b 0

:failed
echo.
echo Inventory failed. See the messages above.
echo.
pause
endlocal
exit /b 1

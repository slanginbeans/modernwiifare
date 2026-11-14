@echo off
rem ---------------------------------------------------------------------------
rem  Modern Wiifare - test report
rem
rem  After playing, put the SD card in this PC and double-click this file. It:
rem    1. finds the card (any drive with apps\mwf_game\logs)
rem    2. copies the game's test logs to out\logs\ (new ones only)
rem    3. summarises them (frame times, CPU breakdown, worst moments, errors,
rem       kills and hits) into out\logs\summary.txt
rem    4. puts the summary on the clipboard: paste it into the chat
rem
rem  The logs hold only timings, counts and events (no game content).
rem
rem    report.bat              newest logs from the card
rem    report.bat X:\path.log  summarise that log instead
rem ---------------------------------------------------------------------------
setlocal enabledelayedexpansion
cd /d "%~dp0"
if not exist "out\logs" mkdir "out\logs"

set "LIST="
if not "%~1"=="" (
    set "LIST="%~1""
    goto summarise
)

set "CARD="
for %%D in (D E F G H I J K L M N O P Q R S T U V W X Y Z) do (
    if not defined CARD if exist "%%D:\apps\mwf_game\logs\" set "CARD=%%D:\apps\mwf_game\logs"
)
if not defined CARD (
    echo [report] No SD card with apps\mwf_game\logs found. Put the card in, or pass a log file.
    echo          ^(Logs appear after playing a match with this build.^)
    goto failed
)
echo [report] Card: %CARD%
rem New logs (not copied before) get summarised; if none are new, the newest one.
for %%F in ("%CARD%\*.log") do (
    if not exist "out\logs\%%~nxF" (
        copy /y "%%F" "out\logs\%%~nxF" >nul
        set "LIST=!LIST! "out\logs\%%~nxF""
    )
)
if not defined LIST (
    for /f "delims=" %%F in ('dir /b /o-n "%CARD%\*.log" 2^>nul') do (
        if not defined LIST (
            copy /y "%CARD%\%%F" "out\logs\%%F" >nul
            set "LIST="out\logs\%%F""
        )
    )
)
if not defined LIST (
    echo [report] The card has no logs yet. Play a match with this build first.
    goto failed
)

:summarise
powershell -NoProfile -ExecutionPolicy Bypass -File "tools\mwf-convert.ps1" report %LIST% > "out\logs\summary.txt"
if errorlevel 1 goto failed
type "out\logs\summary.txt"
clip < "out\logs\summary.txt"
echo.
echo [report] The summary above is on your clipboard ^(also in out\logs\summary.txt^). Paste it into the chat.
echo.
if "%~1"=="" pause
endlocal
exit /b 0

:failed
echo.
if "%~1"=="" pause
endlocal
exit /b 1

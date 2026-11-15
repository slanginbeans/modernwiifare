@echo off
rem ---------------------------------------------------------------------------
rem  Modern Wiifare - builder window
rem
rem  Double-click this file for a window with build.bat's switches as checkboxes
rem  (update, convert game data, slim weapon files, reconvert everything, world
rem  detail, game and devkitPro folders, copy to the SD card), a live log, and
rem  buttons for Build, Clean, Stop, the output folder and the test report.
rem  Settings are remembered in out\builder-settings.json.
rem
rem  build.bat still works on its own, exactly as before.
rem ---------------------------------------------------------------------------
start "" powershell -NoProfile -ExecutionPolicy Bypass -STA -WindowStyle Hidden -File "%~dp0tools\builder-gui.ps1"

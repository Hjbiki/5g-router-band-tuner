@echo off
rem Start the dev app (pnpm tauri dev) from the repo root.
rem Stops a running router-band-tuner.exe first, so there is only one instance.
setlocal
cd /d "%~dp0"

where pnpm >nul 2>nul
if errorlevel 1 (
  echo.
  echo ERROR: pnpm was not found. Install Node 24 and pnpm 10, then try again.
  goto :fail
)

tasklist /FI "IMAGENAME eq router-band-tuner.exe" /NH 2>nul | find /I "router-band-tuner.exe" >nul
if not errorlevel 1 (
  echo Stopping the running router-band-tuner.exe ...
  taskkill /F /T /IM router-band-tuner.exe >nul 2>nul
  if errorlevel 1 (
    echo.
    echo ERROR: could not stop router-band-tuner.exe. Quit it from the tray and try again.
    goto :fail
  )
)

if not exist "node_modules" call pnpm install --frozen-lockfile || goto :installfail
if not exist "ui\node_modules" call pnpm --dir ui install --frozen-lockfile || goto :installfail

echo Starting the dev app ...
call pnpm tauri dev -- --locked
if errorlevel 1 (
  echo.
  echo ERROR: pnpm tauri dev stopped with an error ^(exit code %ERRORLEVEL%^). See the messages above.
  goto :fail
)
exit /b 0

:installfail
echo.
echo ERROR: installing the dependencies failed. See the messages above.

:fail
echo.
pause
exit /b 1

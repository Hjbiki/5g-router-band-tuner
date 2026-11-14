@echo off
rem Build the release app if it is missing or older than the source, then launch it.
rem   target\release\router-band-tuner.exe   (built with: pnpm tauri build --no-bundle)
setlocal
cd /d "%~dp0"
set "EXE=%~dp0target\release\router-band-tuner.exe"

rem Exit code 0: the exe exists and is newer than every source file. 1: it needs a build.
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$exe = Get-Item -LiteralPath $env:EXE -ErrorAction SilentlyContinue;" ^
  "if (-not $exe) { exit 1 };" ^
  "$src = @('crates','src-tauri\src','src-tauri\capabilities','src-tauri\icons','ui\src') | Where-Object { Test-Path $_ } | ForEach-Object { Get-ChildItem -LiteralPath $_ -Recurse -File };" ^
  "$src += @('Cargo.toml','Cargo.lock','src-tauri\Cargo.toml','src-tauri\tauri.conf.json','src-tauri\build.rs','ui\index.html','ui\package.json','ui\pnpm-lock.yaml','ui\vite.config.ts','package.json') | Where-Object { Test-Path $_ } | ForEach-Object { Get-Item -LiteralPath $_ };" ^
  "$newest = ($src | Measure-Object -Property LastWriteTimeUtc -Maximum).Maximum;" ^
  "if ($newest -gt $exe.LastWriteTimeUtc) { exit 1 } else { exit 0 }"
if errorlevel 1 goto :build
echo The release build is up to date.
goto :launch

:build
where pnpm >nul 2>nul
if errorlevel 1 (
  echo.
  echo ERROR: pnpm was not found. Install Node 24 and pnpm 10, then try again.
  goto :fail
)

rem A running copy locks the exe, so stop it before building.
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

echo Building the release app ^(this takes a few minutes^) ...
call pnpm tauri build --no-bundle -- --locked
if errorlevel 1 (
  echo.
  echo ERROR: the release build failed ^(exit code %ERRORLEVEL%^). See the messages above.
  goto :fail
)
if not exist "%EXE%" (
  echo.
  echo ERROR: the build finished but %EXE% is missing.
  goto :fail
)

:launch
echo Launching %EXE%
start "" "%EXE%"
if errorlevel 1 (
  echo.
  echo ERROR: could not start %EXE%.
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

@echo off
rem Cut a release: version bump from Conventional Commits, CHANGELOG, tests, build, tag.
rem   release.bat --dry-run    preview only
rem   release.bat --help       all options
setlocal
node "%~dp0tools\release\release.mjs" %*
exit /b %ERRORLEVEL%

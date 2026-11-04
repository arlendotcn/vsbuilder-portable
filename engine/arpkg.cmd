@echo off
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0arpkg.ps1" %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
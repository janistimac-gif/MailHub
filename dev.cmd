@echo off
REM Zažene MailHub v razvojnem načinu.
REM Na tem racunalniku je v PATH najprej "Rust stable GNU", ki ne zna povezati
REM programa, zato damo rustup (MSVC) orodja na prvo mesto.
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%~dp0"
call npm run tauri dev

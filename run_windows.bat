@echo off
chcp 65001 >nul
cd /d "%~dp0"
title 360Cleaner - Rust 360 Person Remover

if exist "target\release\cleaner360.exe" (
    start "" "target\release\cleaner360.exe"
    exit /b 0
)

where cargo >nul 2>nul
if %errorlevel% equ 0 (
    echo Компиляция и запуск через Cargo...
    cargo run --release
    exit /b 0
)

echo [ОШИБКА] Rust/Cargo не найден! Установите с https://rustup.rs
pause

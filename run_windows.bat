@echo off
chcp 65001 >nul
title 360Cleaner - Удаление человека с 360 фото (Windows)

echo ==========================================================
echo    360Cleaner — Удаление человека с 360° фото (Windows)
echo ==========================================================

cd /d "%~dp0"

where python >nul 2>nul
if %errorlevel% neq 0 (
    echo [ОШИБКА] Python не найден в системе!
    echo Установите Python с официального сайта: https://www.python.org
    echo Обязательно отметьте галочку "Add Python to PATH" при установке.
    pause
    exit /b 1
)

if not exist "venv" (
    echo Создание виртуального окружения (venv)...
    python -m venv venv
)

call venv\Scripts\activate.bat

echo Проверка зависимостей (PySide6, OpenCV, NumPy, Pillow)...
pip install -q -r requirements.txt

echo Запуск приложения...
python main.py

pause

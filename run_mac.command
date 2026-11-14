#!/bin/bash
cd "$(dirname "$0")"

echo "=========================================================="
echo "   360Cleaner — Удаление человека с 360° фото (macOS)"
echo "=========================================================="

# Check Python3
if ! command -v python3 &> /dev/null; then
    echo "Ошибка: Python 3 не найден в системе!"
    echo "Установите Python 3 с https://www.python.org"
    read -p "Нажмите Enter для выхода..."
    exit 1
fi

# Create venv if not exists
if [ ! -d "venv" ]; then
    echo "Создание виртуального окружения (venv)..."
    python3 -m venv venv
fi

# Activate venv
source venv/bin/activate

# Check and install requirements
echo "Проверка зависимостей (PySide6, OpenCV, NumPy, Pillow)..."
pip install -q -r requirements.txt

echo "Запуск приложения..."
python3 main.py

echo "Приложение закрыто."

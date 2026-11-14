#!/bin/bash
cd "$(dirname "$0")"

# Check if compiled binary exists
if [ -f "target/release/cleaner360" ]; then
    ./target/release/cleaner360
    exit 0
fi

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

if command -v cargo &> /dev/null; then
    echo "Запуск через Cargo..."
    cargo run --release
else
    echo "Ошибка: Rust / Cargo не найден!"
    echo "Установите Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    read -p "Нажмите Enter для выхода..."
    exit 1
fi

"""
Main cross-platform entry point for 360Cleaner.
Automatically selects modern Qt GUI (PySide6/PyQt6) with native drag & drop,
or falls back to Tkinter if Qt is not installed.
"""

import sys
import os

# Add project root to sys.path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

def run():
    # Try modern Qt GUI (PySide6 / PyQt6) first
    try:
        import gui_qt
        gui_qt.main()
        return
    except ImportError as e:
        print(f"Qt GUI notice: {e}")
        print("Falling back to Tkinter GUI...")

    # Fallback to Tkinter GUI
    try:
        import app_gui
        app_gui.main()
    except Exception as e:
        print(f"Error launching GUI: {e}")
        print("\nPlease ensure dependencies are installed:")
        print("  pip install -r requirements.txt")
        sys.exit(1)

if __name__ == "__main__":
    run()

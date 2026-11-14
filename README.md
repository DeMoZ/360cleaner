# 360Cleaner — Photographer & People Remover for 360° Panoramas (No AI)

[![Python 3.9+](https://img.shields.io/badge/python-3.9+-blue.svg)](https://www.python.org/downloads/)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-brightgreen.svg)]()
[![License](https://img.shields.io/badge/license-MIT-green.svg)]()

A desktop application for macOS and Windows that merges two 360° equirectangular panoramas shot from the same tripod spot and automatically removes the person/photographer by substituting clean background from the other shot.

Works **100% using classical Computer Vision algorithms (zero AI / no neural networks)**: no hallucinations, preserving authentic source sharpness, texture details, and spherical Photosphere (XMP GPano) metadata.

---

## ✨ Key Features

- 🚀 **Drag & Drop**: Simply drag two 360° photos from Finder (Mac) or File Explorer (Windows) into the window.
- ⚡ **Real-Time Preview**: Fast downsampled preview with instant slider feedback.
- 🔄 **One-Click Swap**: If the person remains visible after automatic cluster assignment, one click inverts the selection.
- 🌐 **360° Equirectangular Wrapping**: Seamless blending across the $0^\circ / 360^\circ$ seam via circular trigonometric embedding $(\cos \theta, \sin \theta)$.
- 🎨 **Seamless Blending (Feathering / Multi-band)**: Eliminates visible borders caused by subtle exposure or cloud shifts between shots.
- 🧭 **Google Photosphere / XMP GPano Metadata Preservation**: Resulting photos are immediately recognized as spherical 360° panoramas by Google Photos, Facebook, VR headsets, and Insta360 Studio.
- 🖥 **Cross-Platform GUI**: Modern dark theme running natively on macOS (Apple Silicon & Intel) and Windows 10/11.

---

## 🚀 Quick Start

### 🍏 macOS
Double-click **`run_mac.command`** in Finder.
*(The script will automatically set up the virtual environment, install dependencies, and launch the app).*

Or via Terminal:
```bash
cd /Users/roman/Git/360cleaner
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt
python3 main.py
```

---

### 🪟 Windows
Double-click **`run_windows.bat`** in File Explorer.

Or via Command Prompt / PowerShell:
```bat
cd 360cleaner
python -m venv venv
call venv\Scripts\activate.bat
pip install -r requirements.txt
python main.py
```

---

## 🎛 Processing Parameters

| Parameter | Description |
| :--- | :--- |
| **Sensitivity (Threshold)** | Motion detection threshold in Lab color space ($\Delta E$). Lower if clothing color matches background; raise if strong wind shakes foliage. |
| **Silhouette & Shadow Dilation (px)** | Morphological expansion radius to completely encapsulate body contours, hair, and cast floor shadows. |
| **Seam Feathering (px)** | Soft transition width to eliminate hard borders between shots. |
| **🔄 Swap Zones** | Inverts the image assignment if the wrong person was kept. |

---

## 🧩 Additional Tools

- **`cli.py`** — Command-line interface for batch or headless automation:
  ```bash
  python cli.py photo1.jpg photo2.jpg -o clean_360.jpg
  ```
- **`web_app.py`** — Local web server with interactive 3D spherical viewer (Pannellum):
  ```bash
  python web_app.py
  # Open in browser: http://127.0.0.1:8080
  ```

---

## 📄 License

MIT License. Free for personal and commercial use.

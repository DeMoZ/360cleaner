# 360Cleaner — Photographer & People Remover for 360° Panoramas (Written in Rust)

[![Rust 1.75+](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-brightgreen.svg)]()
[![License](https://img.shields.io/badge/license-MIT-green.svg)]()

A high-performance desktop application for macOS and Windows written in **Rust** that merges two 360° equirectangular panoramas shot from the same tripod spot and automatically removes the photographer/people by substituting clean background from the other shot.

Works **100% using classical Computer Vision algorithms with multi-threaded Rayon execution (zero AI / no neural networks)**: no hallucinations, authentic source sharpness, and preserved spherical Photosphere (XMP GPano) metadata.

---

## ✨ Features

- 🦀 **100% Pure Rust**: Blazing fast parallel image processing via `rayon` across all CPU cores.
- 🌐 **Interactive 360° Sphere Viewer**: Click the **«🌐 360 Просмотр»** button to instantly switch from the flat panorama into an interactive 3D spherical viewer. Drag with mouse to look in any direction. Press the button again to return to normal flat photo view!
- 🔍 **Zoom & Reset Zoom**:
  - Zoom in (`+`), Zoom out (`-`), or mouse wheel in both 360 mode (adjusts FOV) and 2D flat mode (adjusts scale).
  - **«⟲ Сброс зума»** button immediately resets to the default 100% view.
- 📐 **Popular Aspect Ratio Framing**:
  - `2:1` (Original 360 Equirectangular)
  - `16:9` (Cinema / Widescreen)
  - `4:3` (Standard photo)
  - `1:1` (Square post)
  - `9:16` (Stories / Reels / Shorts / TikTok)
  - `4:5` (Instagram Portrait)
- 🔄 **One-Click Swap**: Inverts the zone assignment if the person was retained.
- 🎯 **Guaranteed Preview-to-Export Consistency**: Cluster assignment uses canonical angular coordinates on the unit circle $S^1$, guaranteeing that what you see in the preview is 100% identical to the full-resolution export (no scanline drift or inverted results).
- 🧭 **Google Photosphere / XMP GPano Metadata**: Final JPEGs retain original spherical metadata so Google Photos, Facebook, VR headsets, and Insta360 Studio continue to treat them as interactive 360° spheres.
- 🚀 **Drag & Drop**: Drop two 360° photos from Finder (Mac) or File Explorer (Windows) directly into the app.

---

## 🚀 Quick Start

### 🍏 macOS
Double-click **`run_mac.command`** in Finder.
*(Launches the precompiled native 1.7MB binary, or compiles via `cargo run --release` if building from source).*

Or via Terminal:
```bash
cd /Users/roman/Git/360cleaner
cargo run --release
```

---

### 🪟 Windows
Double-click **`run_windows.bat`** in File Explorer.

Or via Command Prompt / PowerShell:
```bat
cd 360cleaner
cargo run --release
```

---

## 🎛 Controls & Parameters

| Parameter | Description |
| :--- | :--- |
| **Sensitivity (Threshold)** | Motion detection threshold in Lab color space ($\Delta E$). Lower if clothing color matches background; raise if wind moves outdoor foliage. |
| **Silhouette & Shadow Dilation (px)** | Morphological expansion radius with circular 360 wrapping to encapsulate body contours, hair, and floor shadows. |
| **Seam Feathering (px)** | Soft transition band to eliminate hard seams between frames. |
| **🔄 Swap Zones** | Inverts which image provides the background for each detected cluster. |
| **🌐 View Toggle** | Switches the display between 2D flat panorama view and interactive 360 spherical view. |
| **Aspect Ratio** | Frames the viewport to popular formats (2:1, 16:9, 4:3, 1:1, 9:16, 4:5). |
| **Reset Zoom** | Resets zoom and centers the image. |

---

## 📄 License

MIT License. Free for personal and commercial use.

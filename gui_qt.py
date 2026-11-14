"""
Cross-platform Modern GUI for 360 Photo Person Remover.
Supports macOS and Windows natively with PySide6 / PyQt6.
Features Drag & Drop, real-time preview, multi-threading, and 360 XMP preservation.
"""

import os
import sys
import numpy as np
from PIL import Image

# Support both PySide6 and PyQt6
try:
    from PySide6.QtCore import Qt, QThread, Signal, QSize, QPoint
    from PySide6.QtGui import QImage, QPixmap, QColor, QFont, QIcon, QPainter, QAction
    from PySide6.QtWidgets import (
        QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
        QGridLayout, QLabel, QPushButton, QSlider, QRadioButton, QButtonGroup,
        QFileDialog, QMessageBox, QProgressBar, QFrame, QSplitter, QStatusBar
    )
except ImportError:
    try:
        from PyQt6.QtCore import Qt, QThread, pyqtSignal as Signal, QSize, QPoint
        from PyQt6.QtGui import QImage, QPixmap, QColor, QFont, QIcon, QPainter, QAction
        from PyQt6.QtWidgets import (
            QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
            QGridLayout, QLabel, QPushButton, QSlider, QRadioButton, QButtonGroup,
            QFileDialog, QMessageBox, QProgressBar, QFrame, QSplitter, QStatusBar
        )
    except ImportError:
        print("Error: Neither PySide6 nor PyQt6 is installed.")
        print("Please run: pip install PySide6")
        sys.exit(1)

import core


def numpy_to_qpixmap(arr):
    """Convert numpy RGB array (uint8) to QPixmap."""
    if arr is None:
        return QPixmap()
    h, w, ch = arr.shape
    bytes_per_line = ch * w
    # Create QImage from buffer
    qimg = QImage(arr.data, w, h, bytes_per_line, QImage.Format.Format_RGB888)
    return QPixmap.fromImage(qimg.copy())


class DropZone(QFrame):
    """Interactive Drag & Drop widget with visual state feedback."""
    file_dropped = Signal(str)

    def __init__(self, title, subtitle="Перетащите файл сюда или нажмите для выбора", parent=None):
        super().__init__(parent)
        self.setAcceptDrops(True)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.file_path = None
        self.title_text = title
        self.subtitle_text = subtitle

        self.setObjectName("dropZone")
        layout = QVBoxLayout(self)
        layout.setContentsMargins(16, 20, 16, 20)
        layout.setAlignment(Qt.AlignmentFlag.AlignCenter)

        self.icon_label = QLabel("📷")
        self.icon_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.icon_label.setStyleSheet("font-size: 32px; background: transparent;")
        layout.addWidget(self.icon_label)

        self.title_label = QLabel(title)
        self.title_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.title_label.setStyleSheet("font-weight: bold; font-size: 14px; color: #e2e8f0; background: transparent;")
        layout.addWidget(self.title_label)

        self.subtitle_label = QLabel(subtitle)
        self.subtitle_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.subtitle_label.setStyleSheet("font-size: 11px; color: #94a3b8; background: transparent;")
        layout.addWidget(self.subtitle_label)

        self.file_label = QLabel("")
        self.file_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.file_label.setStyleSheet("font-size: 12px; color: #38bdf8; font-weight: 600; background: transparent;")
        self.file_label.setWordWrap(True)
        layout.addWidget(self.file_label)

        self._apply_normal_style()

    def _apply_normal_style(self):
        self.setStyleSheet("""
            #dropZone {
                border: 2px dashed #475569;
                border-radius: 12px;
                background-color: #1e293b;
            }
            #dropZone:hover {
                border-color: #38bdf8;
                background-color: #243247;
            }
        """)

    def _apply_dragover_style(self):
        self.setStyleSheet("""
            #dropZone {
                border: 2px dashed #38bdf8;
                border-radius: 12px;
                background-color: #0f2d4a;
            }
        """)

    def dragEnterEvent(self, event):
        if event.mimeData().hasUrls():
            event.acceptProposedAction()
            self._apply_dragover_style()

    def dragLeaveEvent(self, event):
        self._apply_normal_style()

    def dropEvent(self, event):
        self._apply_normal_style()
        urls = event.mimeData().urls()
        if urls:
            path = urls[0].toLocalFile()
            if os.path.isfile(path):
                self.set_file(path)
                self.file_dropped.emit(path)

    def mousePressEvent(self, event):
        if event.button() == Qt.MouseButton.LeftButton:
            f, _ = QFileDialog.getOpenFileName(
                self,
                f"Выберите 360° фото: {self.title_text}",
                "",
                "360 Images (*.jpg *.jpeg *.png *.tif *.tiff);;All Files (*.*)"
            )
            if f:
                self.set_file(f)
                self.file_dropped.emit(f)

    def set_file(self, path):
        self.file_path = path
        base_name = os.path.basename(path)
        self.icon_label.setText("✅")
        self.file_label.setText(base_name)
        self.subtitle_label.setText("Файл загружен")


class ResizingImageLabel(QLabel):
    """Aspect-ratio preserving image label for smooth equirectangular 2:1 rendering."""
    def __init__(self, parent=None):
        super().__init__(parent)
        self.pixmap_source = None
        self.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.setStyleSheet("background-color: #0f172a; border-radius: 10px;")

    def set_numpy_image(self, arr):
        if arr is None:
            self.pixmap_source = None
            self.clear()
            self.setText("Загрузите два 360-кадра для предпросмотра")
            self.setStyleSheet("background-color: #0f172a; color: #64748b; font-size: 15px; border-radius: 10px;")
            return

        self.pixmap_source = numpy_to_qpixmap(arr)
        self.update_scaled_pixmap()

    def resizeEvent(self, event):
        super().resizeEvent(event)
        self.update_scaled_pixmap()

    def update_scaled_pixmap(self):
        if self.pixmap_source and not self.pixmap_source.isNull():
            w = self.width() - 8
            h = self.height() - 8
            if w > 10 and h > 10:
                scaled = self.pixmap_source.scaled(
                    w, h,
                    Qt.AspectRatioMode.KeepAspectRatio,
                    Qt.TransformationMode.SmoothTransformation
                )
                self.setPixmap(scaled)


class ProcessingWorker(QThread):
    """Background worker for non-blocking preview pipeline computation."""
    finished = Signal(dict)
    error = Signal(str)

    def __init__(self, path1, path2, threshold, dilate, feather, swap, preview_dim=1200):
        super().__init__()
        self.path1 = path1
        self.path2 = path2
        self.threshold = threshold
        self.dilate = dilate
        self.feather = feather
        self.swap = swap
        self.preview_dim = preview_dim

    def run(self):
        try:
            img1 = core.load_image(self.path1, max_dim=self.preview_dim)
            img2 = core.load_image(self.path2, max_dim=self.preview_dim)

            if img1.shape != img2.shape:
                p2 = Image.fromarray(img2).resize((img1.shape[1], img1.shape[0]))
                img2 = np.array(p2)

            diff_mask = core.compute_difference_mask(
                img1, img2,
                threshold=self.threshold,
                dilate_radius=self.dilate
            )
            mask_A, mask_B, angle_A, angle_B = core.split_clusters_angular(diff_mask, img1.shape[1])

            result = core.blend_images(
                img1, img2,
                mask_A, mask_B,
                swap=self.swap,
                feather_radius=self.feather
            )

            # Mask color visualization
            h, w = diff_mask.shape
            mask_vis = np.zeros((h, w, 3), dtype=np.uint8)
            mask_vis[mask_A > 0] = [56, 189, 248]   # Cyan: Zone A
            mask_vis[mask_B > 0] = [239, 68, 68]    # Red: Zone B

            self.finished.emit({
                "result": result,
                "mask_vis": mask_vis,
                "photo1": img1,
                "photo2": img2,
                "angle_A": angle_A,
                "angle_B": angle_B
            })
        except Exception as e:
            self.error.emit(str(e))


class ExportWorker(QThread):
    """Background worker for rendering full-resolution 360 photo with XMP metadata."""
    finished = Signal(str)
    error = Signal(str)

    def __init__(self, path1, path2, out_path, threshold, dilate, feather, swap):
        super().__init__()
        self.path1 = path1
        self.path2 = path2
        self.out_path = out_path
        self.threshold = threshold
        self.dilate = dilate
        self.feather = feather
        self.swap = swap

    def run(self):
        try:
            core.process_clean_plate(
                self.path1,
                self.path2,
                output_path=self.out_path,
                threshold=self.threshold,
                dilate_radius=self.dilate,
                feather_radius=self.feather,
                swap=self.swap,
                max_dim=None  # Full original resolution
            )
            self.finished.emit(self.out_path)
        except Exception as e:
            self.error.emit(str(e))


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("360Cleaner — Удаление человека с 360° фото (Без ИИ)")
        self.resize(1180, 820)
        self.setMinimumSize(950, 680)

        self.path1 = None
        self.path2 = None
        self.swap_state = False

        self.cached_images = {
            "result": None,
            "mask": None,
            "photo1": None,
            "photo2": None
        }
        self.current_view_mode = "result"

        self.worker = None
        self.export_worker = None

        self._setup_style()
        self._init_ui()

    def _setup_style(self):
        # Universal modern dark theme for macOS & Windows
        self.setStyleSheet("""
            QMainWindow {
                background-color: #0b0f19;
            }
            QWidget {
                color: #e2e8f0;
                font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            }
            QFrame#card {
                background-color: #1e293b;
                border-radius: 12px;
                padding: 12px;
            }
            QLabel {
                background: transparent;
            }
            QPushButton {
                background-color: #2563eb;
                color: #ffffff;
                font-weight: 600;
                font-size: 13px;
                border-radius: 8px;
                padding: 8px 16px;
                border: none;
            }
            QPushButton:hover {
                background-color: #1d4ed8;
            }
            QPushButton:pressed {
                background-color: #1e40af;
            }
            QPushButton#swapBtn {
                background-color: #7c3aed;
            }
            QPushButton#swapBtn:hover {
                background-color: #6d28d9;
            }
            QPushButton#saveBtn {
                background-color: #059669;
                font-size: 14px;
                padding: 10px 20px;
            }
            QPushButton#saveBtn:hover {
                background-color: #047857;
            }
            QSlider::groove:horizontal {
                height: 6px;
                background: #334155;
                border-radius: 3px;
            }
            QSlider::sub-page:horizontal {
                background: #38bdf8;
                border-radius: 3px;
            }
            QSlider::handle:horizontal {
                background: #ffffff;
                border: 2px solid #38bdf8;
                width: 16px;
                margin-top: -5px;
                margin-bottom: -5px;
                border-radius: 8px;
            }
            QRadioButton {
                font-size: 13px;
                font-weight: 500;
                color: #94a3b8;
                spacing: 6px;
            }
            QRadioButton:checked {
                color: #38bdf8;
                font-weight: 600;
            }
            QRadioButton::indicator {
                width: 14px;
                height: 14px;
                border-radius: 7px;
                border: 2px solid #64748b;
                background-color: #0f172a;
            }
            QRadioButton::indicator:checked {
                border: 2px solid #38bdf8;
                background-color: #38bdf8;
            }
            QStatusBar {
                background-color: #0f172a;
                color: #94a3b8;
                font-size: 12px;
            }
        """)

    def _init_ui(self):
        central_widget = QWidget()
        self.setCentralWidget(central_widget)
        main_layout = QVBoxLayout(central_widget)
        main_layout.setContentsMargins(18, 14, 18, 14)
        main_layout.setSpacing(14)

        # 1. Top Section: Drop Zones for Photo 1 and Photo 2
        drop_layout = QHBoxLayout()
        drop_layout.setSpacing(14)

        self.dz1 = DropZone("Фото 1 (Человек на позиции A)", "Перетащите кадр сюда или нажмите для выбора")
        self.dz1.file_dropped.connect(self._on_file1_selected)
        drop_layout.addWidget(self.dz1)

        self.dz2 = DropZone("Фото 2 (Человек на позиции B)", "Перетащите кадр сюда или нажмите для выбора")
        self.dz2.file_dropped.connect(self._on_file2_selected)
        drop_layout.addWidget(self.dz2)

        main_layout.addLayout(drop_layout)

        # Support dropping two files at once anywhere on the window
        self.setAcceptDrops(True)

        # 2. Controls Bar (Sliders + Action Buttons)
        ctrl_card = QFrame()
        ctrl_card.setObjectName("card")
        ctrl_layout = QHBoxLayout(ctrl_card)
        ctrl_layout.setContentsMargins(16, 12, 16, 12)
        ctrl_layout.setSpacing(24)

        # Sliders column
        sliders_box = QVBoxLayout()
        sliders_box.setSpacing(8)

        # Slider: Threshold
        row1 = QHBoxLayout()
        lbl1 = QLabel("Чувствительность (Порог):")
        lbl1.setFixedWidth(200)
        self.slider_thresh = QSlider(Qt.Orientation.Horizontal)
        self.slider_thresh.setRange(5, 60)
        self.slider_thresh.setValue(24)
        self.val_thresh = QLabel("24")
        self.val_thresh.setFixedWidth(30)
        self.slider_thresh.valueChanged.connect(lambda v: (self.val_thresh.setText(str(v)), self.trigger_recompute()))
        row1.addWidget(lbl1)
        row1.addWidget(self.slider_thresh)
        row1.addWidget(self.val_thresh)
        sliders_box.addLayout(row1)

        # Slider: Dilate
        row2 = QHBoxLayout()
        lbl2 = QLabel("Захват силуэта и тени (px):")
        lbl2.setFixedWidth(200)
        self.slider_dilate = QSlider(Qt.Orientation.Horizontal)
        self.slider_dilate.setRange(5, 60)
        self.slider_dilate.setValue(25)
        self.val_dilate = QLabel("25")
        self.val_dilate.setFixedWidth(30)
        self.slider_dilate.valueChanged.connect(lambda v: (self.val_dilate.setText(str(v)), self.trigger_recompute()))
        row2.addWidget(lbl2)
        row2.addWidget(self.slider_dilate)
        row2.addWidget(self.val_dilate)
        sliders_box.addLayout(row2)

        # Slider: Feather
        row3 = QHBoxLayout()
        lbl3 = QLabel("Сглаживание шва (Feather):")
        lbl3.setFixedWidth(200)
        self.slider_feather = QSlider(Qt.Orientation.Horizontal)
        self.slider_feather.setRange(2, 50)
        self.slider_feather.setValue(20)
        self.val_feather = QLabel("20")
        self.val_feather.setFixedWidth(30)
        self.slider_feather.valueChanged.connect(lambda v: (self.val_feather.setText(str(v)), self.trigger_recompute()))
        row3.addWidget(lbl3)
        row3.addWidget(self.slider_feather)
        row3.addWidget(self.val_feather)
        sliders_box.addLayout(row3)

        ctrl_layout.addLayout(sliders_box, stretch=3)

        # Buttons column
        btn_box = QVBoxLayout()
        btn_box.setSpacing(10)
        btn_box.setAlignment(Qt.AlignmentFlag.AlignCenter)

        self.btn_swap = QPushButton("🔄 Поменять местами человека")
        self.btn_swap.setObjectName("swapBtn")
        self.btn_swap.setToolTip("Инвертировать замену, если человек остался в кадре")
        self.btn_swap.clicked.connect(self._toggle_swap)
        btn_box.addWidget(self.btn_swap)

        self.btn_save = QPushButton("💾 Сохранить готовое 360° фото")
        self.btn_save.setObjectName("saveBtn")
        self.btn_save.setToolTip("Сохранить панораму в полном разрешении с метаданными GPano / XMP")
        self.btn_save.clicked.connect(self._save_result)
        btn_box.addWidget(self.btn_save)

        ctrl_layout.addLayout(btn_box, stretch=2)
        main_layout.addWidget(ctrl_card)

        # 3. View Switcher Mode Bar
        view_bar = QHBoxLayout()
        view_bar.setContentsMargins(6, 0, 6, 0)
        view_bar.setSpacing(18)

        lbl_mode = QLabel("Режим просмотра:")
        lbl_mode.setStyleSheet("font-weight: bold; color: #f1f5f9; font-size: 13px;")
        view_bar.addWidget(lbl_mode)

        self.bg_modes = QButtonGroup(self)
        modes = [
            ("🎯 Чистый результат (Без человека)", "result"),
            ("🎭 Карта различий (Маска)", "mask"),
            ("📸 Фото 1 (Исходник)", "photo1"),
            ("📸 Фото 2 (Исходник)", "photo2"),
        ]
        for idx, (label, mode_id) in enumerate(modes):
            rb = QRadioButton(label)
            if idx == 0:
                rb.setChecked(True)
            self.bg_modes.addButton(rb, idx)
            rb.toggled.connect(lambda checked, m=mode_id: self._on_mode_toggled(m, checked))
            view_bar.addWidget(rb)

        view_bar.addStretch()
        main_layout.addLayout(view_bar)

        # 4. Preview Canvas Area
        self.image_display = ResizingImageLabel()
        main_layout.addWidget(self.image_display, stretch=1)

        # 5. Status Bar
        self.statusBar = QStatusBar()
        self.setStatusBar(self.statusBar)
        self.statusBar.showMessage("Готов к работе. Перетащите две 360-градусные фотографии.")

    def dragEnterEvent(self, event):
        if event.mimeData().hasUrls():
            event.acceptProposedAction()

    def dropEvent(self, event):
        urls = event.mimeData().urls()
        files = [u.toLocalFile() for u in urls if os.path.isfile(u.toLocalFile())]
        if len(files) >= 2:
            self._on_file1_selected(files[0])
            self._on_file2_selected(files[1])
            self.dz1.set_file(files[0])
            self.dz2.set_file(files[1])
        elif len(files) == 1:
            if not self.path1:
                self._on_file1_selected(files[0])
                self.dz1.set_file(files[0])
            else:
                self._on_file2_selected(files[0])
                self.dz2.set_file(files[0])

    def _on_file1_selected(self, path):
        self.path1 = path
        self.trigger_recompute()

    def _on_file2_selected(self, path):
        self.path2 = path
        self.trigger_recompute()

    def _toggle_swap(self):
        self.swap_state = not self.swap_state
        self.statusBar.showMessage(f"Инвертировано назначение зон (Swap = {self.swap_state})")
        self.trigger_recompute()

    def _on_mode_toggled(self, mode_id, checked):
        if checked:
            self.current_view_mode = mode_id
            self._refresh_display()

    def trigger_recompute(self):
        if not self.path1 or not self.path2:
            return

        self.statusBar.showMessage("Сведение панорам и устранение человека...")

        thresh = self.slider_thresh.value()
        dilate = self.slider_dilate.value()
        feather = self.slider_feather.value()
        swap = self.swap_state

        if self.worker is not None and self.worker.isRunning():
            self.worker.terminate()
            self.worker.wait()

        self.worker = ProcessingWorker(self.path1, self.path2, thresh, dilate, feather, swap)
        self.worker.finished.connect(self._on_processing_finished)
        self.worker.error.connect(self._on_processing_error)
        self.worker.start()

    def _on_processing_finished(self, data):
        self.cached_images["result"] = data["result"]
        self.cached_images["mask"] = data["mask_vis"]
        self.cached_images["photo1"] = data["photo1"]
        self.cached_images["photo2"] = data["photo2"]

        ang_a = data.get("angle_A", 0.0)
        ang_b = data.get("angle_B", 180.0)
        self.statusBar.showMessage(
            f"Готово! Человек удален. Обнаружен человек на углах: A={ang_a:.0f}°, B={ang_b:.0f}°"
        )
        self._refresh_display()

    def _on_processing_error(self, err_msg):
        self.statusBar.showMessage(f"Ошибка обработки: {err_msg}")
        QMessageBox.warning(self, "Ошибка обработки", f"Не удалось свести кадры:\n{err_msg}")

    def _refresh_display(self):
        target = self.cached_images.get(self.current_view_mode)
        self.image_display.set_numpy_image(target)

    def _save_result(self):
        if not self.path1 or not self.path2:
            QMessageBox.information(self, "Внимание", "Сначала выберите два 360° фото.")
            return

        out_path, _ = QFileDialog.getSaveFileName(
            self,
            "Сохранить чистое 360° фото",
            "clean_360_panorama.jpg",
            "JPEG 360 Image (*.jpg *.jpeg);;PNG Image (*.png)"
        )
        if not out_path:
            return

        self.btn_save.setEnabled(False)
        self.statusBar.showMessage("Рендеринг финального 360° фото в полном разрешении...")

        thresh = self.slider_thresh.value()
        dilate = self.slider_dilate.value()
        feather = self.slider_feather.value()
        swap = self.swap_state

        self.export_worker = ExportWorker(
            self.path1, self.path2, out_path, thresh, dilate, feather, swap
        )
        self.export_worker.finished.connect(self._on_save_finished)
        self.export_worker.error.connect(self._on_save_error)
        self.export_worker.start()

    def _on_save_finished(self, saved_path):
        self.btn_save.setEnabled(True)
        self.statusBar.showMessage(f"Успешно сохранено: {os.path.basename(saved_path)}")
        QMessageBox.information(
            self,
            "Успех!",
            f"360° панорама успешно очищена и сохранена:\n{saved_path}\n\n"
            "Метаданные 360-сферы (Photosphere XMP/GPano) сохранены."
        )

    def _on_save_error(self, err_msg):
        self.btn_save.setEnabled(True)
        self.statusBar.showMessage(f"Ошибка сохранения: {err_msg}")
        QMessageBox.critical(self, "Ошибка сохранения", f"Не удалось сохранить фото:\n{err_msg}")


def main():
    # Enable High DPI scaling for Retina Mac and 4K Windows
    app = QApplication(sys.argv)
    app.setApplicationName("360Cleaner")

    window = MainWindow()
    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()

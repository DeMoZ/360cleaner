"""
Desktop GUI application for 360-degree panorama person removal.
Uses Tkinter for cross-platform native UI on macOS, Windows, and Linux.
No AI required.
"""

import os
import sys
import threading
import tkinter as tk
from tkinter import ttk, filedialog, messagebox
from PIL import Image, ImageTk

# Import processing core
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import core


class PanoramaCleanerApp(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("360 Photo Person Remover (Non-AI)")
        self.geometry("1100x780")
        self.minsize(900, 650)

        # State
        self.path1 = None
        self.path2 = None
        self.preview_dim = 1200  # fast preview resolution

        self.img1_preview = None
        self.img2_preview = None
        self.result_preview = None
        self.mask_preview = None

        self.mask_A = None
        self.mask_B = None
        self.angle_A = 0.0
        self.angle_B = 180.0

        self.swap_var = tk.BooleanVar(value=False)
        self.threshold_var = tk.IntVar(value=24)
        self.dilate_var = tk.IntVar(value=25)
        self.feather_var = tk.IntVar(value=20)
        self.view_mode_var = tk.StringVar(value="result")

        self.processing_lock = threading.Lock()
        self.is_busy = False

        self._build_ui()

    def _build_ui(self):
        # Configure style
        style = ttk.Style(self)
        try:
            style.theme_use("clam")
        except:
            pass

        # Top Control / File Selection Frame
        top_frame = ttk.LabelFrame(self, text=" 1. Выберите две 360° фотографии со штатива ", padding=10)
        top_frame.pack(fill=tk.X, padx=12, pady=6)

        # File 1 column
        f1_box = ttk.Frame(top_frame)
        f1_box.pack(side=tk.LEFT, fill=tk.X, expand=True, padx=8)

        self.btn_f1 = ttk.Button(f1_box, text="📂 Выбрать Фото 1 (Человек на позиции A)", command=self.choose_file_1)
        self.btn_f1.pack(fill=tk.X, pady=2)
        self.lbl_f1 = ttk.Label(f1_box, text="Файл не выбран", font=("Helvetica", 10), foreground="#666666")
        self.lbl_f1.pack(fill=tk.X)

        # File 2 column
        f2_box = ttk.Frame(top_frame)
        f2_box.pack(side=tk.LEFT, fill=tk.X, expand=True, padx=8)

        self.btn_f2 = ttk.Button(f2_box, text="📂 Выбрать Фото 2 (Человек на позиции B)", command=self.choose_file_2)
        self.btn_f2.pack(fill=tk.X, pady=2)
        self.lbl_f2 = ttk.Label(f2_box, text="Файл не выбран", font=("Helvetica", 10), foreground="#666666")
        self.lbl_f2.pack(fill=tk.X)

        # Middle Options & Tuning Frame
        opts_frame = ttk.LabelFrame(self, text=" 2. Настройки сведения и устранения человека ", padding=10)
        opts_frame.pack(fill=tk.X, padx=12, pady=4)

        # Sliders grid
        slider_frame = ttk.Frame(opts_frame)
        slider_frame.pack(side=tk.LEFT, fill=tk.X, expand=True)

        # Threshold slider
        r1 = ttk.Frame(slider_frame)
        r1.pack(fill=tk.X, pady=2)
        ttk.Label(r1, text="Чувствительность (Порог):", width=25).pack(side=tk.LEFT)
        self.scale_thresh = ttk.Scale(r1, from_=5, to=60, variable=self.threshold_var, command=self._on_param_change)
        self.scale_thresh.pack(side=tk.LEFT, fill=tk.X, expand=True, padx=6)
        self.lbl_thresh_val = ttk.Label(r1, text=str(self.threshold_var.get()), width=4)
        self.lbl_thresh_val.pack(side=tk.LEFT)

        # Dilate slider
        r2 = ttk.Frame(slider_frame)
        r2.pack(fill=tk.X, pady=2)
        ttk.Label(r2, text="Захват силуэта и тени (px):", width=25).pack(side=tk.LEFT)
        self.scale_dilate = ttk.Scale(r2, from_=5, to=60, variable=self.dilate_var, command=self._on_param_change)
        self.scale_dilate.pack(side=tk.LEFT, fill=tk.X, expand=True, padx=6)
        self.lbl_dilate_val = ttk.Label(r2, text=str(self.dilate_var.get()), width=4)
        self.lbl_dilate_val.pack(side=tk.LEFT)

        # Feather slider
        r3 = ttk.Frame(slider_frame)
        r3.pack(fill=tk.X, pady=2)
        ttk.Label(r3, text="Сглаживание шва (Feather):", width=25).pack(side=tk.LEFT)
        self.scale_feather = ttk.Scale(r3, from_=2, to=50, variable=self.feather_var, command=self._on_param_change)
        self.scale_feather.pack(side=tk.LEFT, fill=tk.X, expand=True, padx=6)
        self.lbl_feather_val = ttk.Label(r3, text=str(self.feather_var.get()), width=4)
        self.lbl_feather_val.pack(side=tk.LEFT)

        # Swap button frame
        action_box = ttk.Frame(opts_frame, padding=5)
        action_box.pack(side=tk.RIGHT, padx=10)

        self.btn_swap = ttk.Button(
            action_box,
            text="🔄 Поменять местами человека\n(Если остался в кадре)",
            command=self.toggle_swap,
            width=28
        )
        self.btn_swap.pack(pady=4)

        # Save button
        self.btn_save = ttk.Button(
            action_box,
            text="💾 Сохранить готовое 360° фото",
            command=self.save_result,
            width=28
        )
        self.btn_save.pack(pady=4)

        # View Mode Selector Frame
        view_bar = ttk.Frame(self, padding=4)
        view_bar.pack(fill=tk.X, padx=12)

        ttk.Label(view_bar, text="Режим просмотра: ", font=("Helvetica", 11, "bold")).pack(side=tk.LEFT, padx=4)
        modes = [
            ("Чистый результат (Без человека)", "result"),
            ("Маска различий (Где были люди)", "mask"),
            ("Исходное Фото 1", "photo1"),
            ("Исходное Фото 2", "photo2"),
        ]
        for label, val in modes:
            ttk.Radiobutton(
                view_bar,
                text=label,
                value=val,
                variable=self.view_mode_var,
                command=self.update_canvas_display
            ).pack(side=tk.LEFT, padx=10)

        # Canvas Area for Preview
        canvas_frame = ttk.Frame(self)
        canvas_frame.pack(fill=tk.BOTH, expand=True, padx=12, pady=6)

        self.canvas = tk.Canvas(canvas_frame, bg="#1e1e1e", highlightthickness=0)
        self.canvas.pack(fill=tk.BOTH, expand=True)
        self.canvas.bind("<Configure>", lambda e: self.update_canvas_display())

        # Status Bar
        self.status_var = tk.StringVar(value="Готов к работе. Выберите две 360-градусные фотографии.")
        status_bar = ttk.Label(self, textvariable=self.status_var, relief=tk.SUNKEN, anchor=tk.W, padding=4)
        status_bar.pack(fill=tk.X, side=tk.BOTTOM)

    def choose_file_1(self):
        f = filedialog.askopenfilename(
            title="Выберите первое 360 фото",
            filetypes=[("360 Images", "*.jpg *.jpeg *.png *.tif *.tiff"), ("All files", "*.*")]
        )
        if f:
            self.path1 = f
            self.lbl_f1.config(text=os.path.basename(f))
            self.trigger_recomputation()

    def choose_file_2(self):
        f = filedialog.askopenfilename(
            title="Выберите второе 360 фото",
            filetypes=[("360 Images", "*.jpg *.jpeg *.png *.tif *.tiff"), ("All files", "*.*")]
        )
        if f:
            self.path2 = f
            self.lbl_f2.config(text=os.path.basename(f))
            self.trigger_recomputation()

    def _on_param_change(self, *args):
        self.lbl_thresh_val.config(text=str(int(self.threshold_var.get())))
        self.lbl_dilate_val.config(text=str(int(self.dilate_var.get())))
        self.lbl_feather_val.config(text=str(int(self.feather_var.get())))
        self.trigger_recomputation()

    def toggle_swap(self):
        self.swap_var.set(not self.swap_var.get())
        self.status_var.set(f"Инвертирована подстановка человека (Swap = {self.swap_var.get()})")
        self.trigger_recomputation()

    def trigger_recomputation(self):
        if not self.path1 or not self.path2:
            return

        threading.Thread(target=self._compute_pipeline_thread, daemon=True).start()

    def _compute_pipeline_thread(self):
        with self.processing_lock:
            self.status_var.set("Сведение 360 фото и устранение человека...")
            try:
                # Load preview downscaled images
                if self.img1_preview is None or self.img2_preview is None:
                    self.img1_preview = core.load_image(self.path1, max_dim=self.preview_dim)
                    self.img2_preview = core.load_image(self.path2, max_dim=self.preview_dim)

                    # Ensure dimensions match
                    if self.img1_preview.shape != self.img2_preview.shape:
                        p2 = Image.fromarray(self.img2_preview).resize(
                            (self.img1_preview.shape[1], self.img1_preview.shape[0])
                        )
                        self.img2_preview = np.array(p2)

                thresh = int(self.threshold_var.get())
                dilate_r = int(self.dilate_var.get())
                feather_r = int(self.feather_var.get())
                swap = self.swap_var.get()

                # Calculate difference and separate clusters
                diff_mask = core.compute_difference_mask(
                    self.img1_preview,
                    self.img2_preview,
                    threshold=thresh,
                    dilate_radius=dilate_r
                )
                self.mask_A, self.mask_B, self.angle_A, self.angle_B = core.split_clusters_angular(
                    diff_mask,
                    self.img1_preview.shape[1]
                )

                # Blend
                self.result_preview = core.blend_images(
                    self.img1_preview,
                    self.img2_preview,
                    self.mask_A,
                    self.mask_B,
                    swap=swap,
                    feather_radius=feather_r
                )

                # Visualize Mask: Zone A (Blue) vs Zone B (Red)
                h, w = diff_mask.shape
                mask_vis = np.zeros((h, w, 3), dtype=np.uint8)
                mask_vis[self.mask_A > 0] = [0, 180, 255]   # Zone A
                mask_vis[self.mask_B > 0] = [255, 60, 60]   # Zone B
                self.mask_preview = mask_vis

                self.after(0, self._on_pipeline_done)
            except Exception as e:
                self.after(0, lambda: self.status_var.set(f"Ошибка: {e}"))

    def _on_pipeline_done(self):
        self.status_var.set("Готово! Фото сведено. Человек удален с обеих зон.")
        self.update_canvas_display()

    def update_canvas_display(self):
        mode = self.view_mode_var.get()
        target_img = None
        if mode == "result":
            target_img = self.result_preview
        elif mode == "mask":
            target_img = self.mask_preview
        elif mode == "photo1":
            target_img = self.img1_preview
        elif mode == "photo2":
            target_img = self.img2_preview

        self.canvas.delete("all")
        if target_img is None:
            cw = self.canvas.winfo_width() or 800
            ch = self.canvas.winfo_height() or 500
            self.canvas.create_text(
                cw // 2,
                ch // 2,
                text="Загрузите два 360-фото для предварительного просмотра",
                fill="#888888",
                font=("Helvetica", 14)
            )
            return

        cw = self.canvas.winfo_width()
        ch = self.canvas.winfo_height()
        if cw < 50 or ch < 50:
            return

        # Fit image maintaining 2:1 aspect ratio
        ih, iw = target_img.shape[:2]
        scale = min(cw / iw, ch / ih)
        nw, nh = max(1, int(iw * scale)), max(1, int(ih * scale))

        pil_img = Image.fromarray(target_img).resize((nw, nh), Image.Resampling.BILINEAR)
        self.tk_photo = ImageTk.PhotoImage(pil_img)

        x = (cw - nw) // 2
        y = (ch - nh) // 2
        self.canvas.create_image(x, y, anchor=tk.NW, image=self.tk_photo)

    def save_result(self):
        if not self.path1 or not self.path2:
            messagebox.showwarning("Внимание", "Сначала выберите оба фото.")
            return

        out_path = filedialog.asksaveasfilename(
            title="Сохранить чистое 360° фото",
            defaultextension=".jpg",
            filetypes=[("JPEG Image", "*.jpg *.jpeg"), ("PNG Image", "*.png")]
        )
        if not out_path:
            return

        # Disable buttons during high-res render
        self.btn_save.config(state=tk.DISABLED)
        self.status_var.set("Рендеринг финального 360° фото в полном разрешении...")

        def _worker():
            try:
                thresh = int(self.threshold_var.get())
                dilate_r = int(self.dilate_var.get())
                feather_r = int(self.feather_var.get())
                swap = self.swap_var.get()

                core.process_clean_plate(
                    self.path1,
                    self.path2,
                    output_path=out_path,
                    threshold=thresh,
                    dilate_radius=dilate_r,
                    feather_radius=feather_r,
                    swap=swap,
                    max_dim=None  # Full original resolution
                )
                self.after(0, lambda: self._on_save_success(out_path))
            except Exception as e:
                self.after(0, lambda: self._on_save_error(str(e)))

        threading.Thread(target=_worker, daemon=True).start()

    def _on_save_success(self, path):
        self.btn_save.config(state=tk.NORMAL)
        self.status_var.set(f"Успешно сохранено: {os.path.basename(path)}")
        messagebox.showinfo(
            "Готово!",
            f"360° панорама успешно очищена и сохранена:\n{path}\n\n"
            f"Метаданные 360-панорамы (GPano / XMP) сохранены!"
        )

    def _on_save_error(self, err_msg):
        self.btn_save.config(state=tk.NORMAL)
        self.status_var.set(f"Ошибка сохранения: {err_msg}")
        messagebox.showerror("Ошибка", f"Не удалось сохранить фото:\n{err_msg}")


def main():
    app = PanoramaCleanerApp()
    app.mainloop()


if __name__ == "__main__":
    main()

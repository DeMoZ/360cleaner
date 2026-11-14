"""
Local web-based 360 photo remover with Drag & Drop and Interactive 360 Sphere Viewer.
Runs locally without third-party web frameworks (uses Python http.server).
"""

import os
import sys
import json
import base64
import cgi
import io
import urllib.parse
from http.server import HTTPServer, BaseHTTPRequestHandler
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import core

HTML_TEMPLATE = """<!DOCTYPE html>
<html lang="ru">
<head>
<meta charset="utf-8">
<title>360° Panorama Person Remover (Без ИИ)</title>
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/pannellum@2.5.6/build/pannellum.css"/>
<script type="text/javascript" src="https://cdn.jsdelivr.net/npm/pannellum@2.5.6/build/pannellum.js"></script>
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; background: #121316; color: #e5e7eb; padding: 24px; }
  .container { max-width: 1200px; margin: 0 auto; }
  header { text-align: center; margin-bottom: 24px; }
  h1 { font-size: 26px; color: #60a5fa; margin-bottom: 8px; }
  p.subtitle { color: #9ca3af; font-size: 14px; }
  
  .drop-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 20px; margin-bottom: 20px; }
  .dropzone {
    border: 2px dashed #3b82f6; border-radius: 12px; padding: 28px 16px; text-align: center;
    background: #1e222a; cursor: pointer; transition: 0.2s all; position: relative;
  }
  .dropzone:hover, .dropzone.dragover { background: #252d3a; border-color: #60a5fa; }
  .dropzone input { display: none; }
  .dropzone .icon { font-size: 32px; margin-bottom: 8px; }
  .dropzone .filename { margin-top: 8px; font-size: 13px; color: #10b981; font-weight: 500; word-break: break-all; }

  .controls-card {
    background: #1e222a; border-radius: 12px; padding: 18px 24px; margin-bottom: 20px;
    display: flex; flex-wrap: wrap; gap: 24px; align-items: center; justify-content: space-between;
  }
  .slider-group { display: flex; align-items: center; gap: 12px; font-size: 14px; }
  .slider-group label { min-width: 160px; color: #d1d5db; }
  input[type="range"] { accent-color: #3b82f6; width: 140px; }

  .btn {
    background: #2563eb; color: #fff; border: none; border-radius: 8px;
    padding: 10px 18px; font-size: 14px; font-weight: 600; cursor: pointer; transition: 0.15s all;
  }
  .btn:hover { background: #1d4ed8; }
  .btn-swap { background: #7c3aed; }
  .btn-swap:hover { background: #6d28d9; }
  .btn-save { background: #059669; }
  .btn-save:hover { background: #047857; }

  .viewer-card {
    background: #1e222a; border-radius: 12px; padding: 16px; margin-bottom: 24px;
  }
  .view-tabs { display: flex; gap: 12px; margin-bottom: 12px; }
  .tab-btn {
    background: #2b3240; color: #9ca3af; border: none; padding: 8px 16px;
    border-radius: 6px; cursor: pointer; font-size: 13px; font-weight: 500;
  }
  .tab-btn.active { background: #3b82f6; color: #fff; }

  #panorama-container { width: 100%; height: 520px; border-radius: 8px; overflow: hidden; background: #000; position: relative; }
  #flat-image { width: 100%; height: 520px; object-fit: contain; display: none; border-radius: 8px; background: #000; }
  .loading-overlay {
    position: absolute; top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(0,0,0,0.7); display: none; align-items: center; justify-content: center;
    font-size: 18px; color: #60a5fa; font-weight: 500; border-radius: 8px; z-index: 10;
  }
</style>
</head>
<body>
<div class="container">
  <header>
    <h1>360° Очиститель от человека (Без ИИ)</h1>
    <p class="subtitle">Перетащите два 360° кадра со штатива. Человек будет автоматически удален с обоих ракурсов.</p>
  </header>

  <div class="drop-grid">
    <div class="dropzone" id="dz1" onclick="document.getElementById('file1').click()">
      <div class="icon">📷</div>
      <b>Фото 1 (Человек на позиции A)</b>
      <p style="font-size:12px; color:#888;">Перетащите файл сюда или нажмите для выбора</p>
      <input type="file" id="file1" accept="image/*">
      <div class="filename" id="f1_name"></div>
    </div>

    <div class="dropzone" id="dz2" onclick="document.getElementById('file2').click()">
      <div class="icon">📷</div>
      <b>Фото 2 (Человек на позиции B)</b>
      <p style="font-size:12px; color:#888;">Перетащите файл сюда или нажмите для выбора</p>
      <input type="file" id="file2" accept="image/*">
      <div class="filename" id="f2_name"></div>
    </div>
  </div>

  <div class="controls-card">
    <div style="display:flex; flex-direction:column; gap:10px;">
      <div class="slider-group">
        <label>Чувствительность (Порог): <span id="v_th">24</span></label>
        <input type="range" id="thresh" min="5" max="60" value="24" oninput="document.getElementById('v_th').innerText=this.value; triggerProcess();">
      </div>
      <div class="slider-group">
        <label>Радиус силуэта/тени: <span id="v_dil">25</span> px</label>
        <input type="range" id="dilate" min="5" max="60" value="25" oninput="document.getElementById('v_dil').innerText=this.value; triggerProcess();">
      </div>
      <div class="slider-group">
        <label>Сглаживание шва (Feather): <span id="v_fea">20</span> px</label>
        <input type="range" id="feather" min="2" max="50" value="20" oninput="document.getElementById('v_fea').innerText=this.value; triggerProcess();">
      </div>
    </div>

    <div style="display:flex; flex-direction:column; gap:12px;">
      <button class="btn btn-swap" onclick="toggleSwap()">🔄 Поменять местами человека</button>
      <button class="btn btn-save" onclick="downloadResult()">💾 Скачать готовое 360° фото</button>
    </div>
  </div>

  <div class="viewer-card">
    <div class="view-tabs">
      <button class="tab-btn active" onclick="switchView('3d')">🌐 3D Интерактивная 360-Сфера</button>
      <button class="tab-btn" onclick="switchView('flat')">🖼 Плоская панорама (2:1)</button>
      <button class="tab-btn" onclick="switchView('mask')">🎭 Маска различий</button>
    </div>
    <div style="position:relative;">
      <div id="panorama-container"></div>
      <img id="flat-image" alt="360 view">
      <div class="loading-overlay" id="loader">Сведение панорам и удаление человека...</div>
    </div>
  </div>
</div>

<script>
let file1 = null, file2 = null;
let currentSwap = false;
let currentResultUrl = null;
let currentMaskUrl = null;
let panoViewer = null;
let currentMode = '3d';
let processDebounce = null;

function setupDrop(dzId, fileInputId, nameId, setFile) {
  const dz = document.getElementById(dzId);
  const fi = document.getElementById(fileInputId);
  dz.addEventListener('dragover', (e) => { e.preventDefault(); dz.classList.add('dragover'); });
  dz.addEventListener('dragleave', () => dz.classList.remove('dragover'));
  dz.addEventListener('drop', (e) => {
    e.preventDefault(); dz.classList.remove('dragover');
    if (e.dataTransfer.files.length > 0) {
      setFile(e.dataTransfer.files[0]);
      document.getElementById(nameId).innerText = e.dataTransfer.files[0].name;
      triggerProcess();
    }
  });
  fi.addEventListener('change', () => {
    if (fi.files.length > 0) {
      setFile(fi.files[0]);
      document.getElementById(nameId).innerText = fi.files[0].name;
      triggerProcess();
    }
  });
}

setupDrop('dz1', 'file1', 'f1_name', f => file1 = f);
setupDrop('dz2', 'file2', 'f2_name', f => file2 = f);

function toggleSwap() {
  currentSwap = !currentSwap;
  triggerProcess();
}

function triggerProcess() {
  if (!file1 || !file2) return;
  clearTimeout(processDebounce);
  processDebounce = setTimeout(runProcess, 250);
}

async function runProcess() {
  document.getElementById('loader').style.display = 'flex';
  const fd = new FormData();
  fd.append('photo1', file1);
  fd.append('photo2', file2);
  fd.append('threshold', document.getElementById('thresh').value);
  fd.append('dilate', document.getElementById('dilate').value);
  fd.append('feather', document.getElementById('feather').value);
  fd.append('swap', currentSwap ? 'true' : 'false');

  try {
    const res = await fetch('/api/process', { method: 'POST', body: fd });
    const data = await res.json();
    if (data.error) {
      alert("Ошибка: " + data.error);
      return;
    }
    currentResultUrl = data.result_url;
    currentMaskUrl = data.mask_url;
    renderCurrentView();
  } catch (err) {
    console.error(err);
  } finally {
    document.getElementById('loader').style.display = 'none';
  }
}

function switchView(mode) {
  currentMode = mode;
  document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
  if (event) event.target.classList.add('active');
  renderCurrentView();
}

function renderCurrentView() {
  const panoDiv = document.getElementById('panorama-container');
  const flatImg = document.getElementById('flat-image');

  if (currentMode === '3d' && currentResultUrl) {
    panoDiv.style.display = 'block';
    flatImg.style.display = 'none';
    panoDiv.innerHTML = '';
    pannellum.viewer('panorama-container', {
      type: 'equirectangular',
      panorama: currentResultUrl,
      autoLoad: true,
      showControls: true
    });
  } else if (currentMode === 'flat' && currentResultUrl) {
    panoDiv.style.display = 'none';
    flatImg.style.display = 'block';
    flatImg.src = currentResultUrl;
  } else if (currentMode === 'mask' && currentMaskUrl) {
    panoDiv.style.display = 'none';
    flatImg.style.display = 'block';
    flatImg.src = currentMaskUrl;
  }
}

function downloadResult() {
  if (!currentResultUrl) {
    alert("Сначала загрузите два фото!");
    return;
  }
  const a = document.createElement('a');
  a.href = currentResultUrl;
  a.download = 'clean_360_panorama.jpg';
  a.click();
}
</script>
</body>
</html>
"""


class WebHandler(BaseHTTPRequestHandler):
    cached_images = {}

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path == "/" or parsed.path == "/index.html":
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(HTML_TEMPLATE.encode("utf-8"))
        elif parsed.path.startswith("/img/"):
            img_id = parsed.path.split("/img/")[1]
            if img_id in self.cached_images:
                data = self.cached_images[img_id]
                self.send_response(200)
                self.send_header("Content-Type", "image/jpeg")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            else:
                self.send_response(404)
                self.end_headers()
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        if self.path == "/api/process":
            ctype = self.headers.get("Content-Type", "")
            if not ctype.startswith("multipart/form-data"):
                self.send_error(400, "Expected multipart/form-data")
                return

            # Parse multipart form data
            form = cgi.FieldStorage(
                fp=self.rfile,
                headers=self.headers,
                environ={"REQUEST_METHOD": "POST", "CONTENT_TYPE": self.headers["Content-Type"]}
            )

            file1_item = form["photo1"]
            file2_item = form["photo2"]
            thresh = int(form.getvalue("threshold", 24))
            dilate_r = int(form.getvalue("dilate", 25))
            feather_r = int(form.getvalue("feather", 20))
            swap = form.getvalue("swap", "false").lower() == "true"

            # Load images
            img1 = Image.open(io.BytesIO(file1_item.file.read())).convert("RGB")
            img2 = Image.open(io.BytesIO(file2_item.file.read())).convert("RGB")

            # Scale for responsive web preview (max width 1920)
            img1_arr = core.load_image_from_pil(img1, max_dim=1920)
            img2_arr = core.load_image_from_pil(img2, max_dim=1920)

            if img1_arr.shape != img2_arr.shape:
                p2 = Image.fromarray(img2_arr).resize((img1_arr.shape[1], img1_arr.shape[0]))
                img2_arr = np.array(p2)

            diff_mask = core.compute_difference_mask(img1_arr, img2_arr, threshold=thresh, dilate_radius=dilate_r)
            mask_A, mask_B, angle_A, angle_B = core.split_clusters_angular(diff_mask, img1_arr.shape[1])
            result_arr = core.blend_images(img1_arr, img2_arr, mask_A, mask_B, swap=swap, feather_radius=feather_r)

            # Mask preview
            h, w = diff_mask.shape
            mask_vis = np.zeros((h, w, 3), dtype=np.uint8)
            mask_vis[mask_A > 0] = [0, 180, 255]
            mask_vis[mask_B > 0] = [255, 60, 60]

            # Save to buffer
            buf_res = io.BytesIO()
            Image.fromarray(result_arr).save(buf_res, format="JPEG", quality=95)
            res_bytes = buf_res.getvalue()

            buf_mask = io.BytesIO()
            Image.fromarray(mask_vis).save(buf_mask, format="JPEG", quality=90)
            mask_bytes = buf_mask.getvalue()

            res_id = f"res_{os.urandom(4).hex()}.jpg"
            mask_id = f"mask_{os.urandom(4).hex()}.jpg"
            self.cached_images[res_id] = res_bytes
            self.cached_images[mask_id] = mask_bytes

            resp = {
                "result_url": f"/img/{res_id}",
                "mask_url": f"/img/{mask_id}",
                "angle_A": angle_A,
                "angle_B": angle_B
            }

            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(resp).encode("utf-8"))


def main():
    port = 8080
    server = HTTPServer(("127.0.0.1", port), WebHandler)
    print(f"360 Cleaner Web Server running at http://127.0.0.1:{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopping server.")


if __name__ == "__main__":
    main()

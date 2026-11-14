//! 360Cleaner — Fast, Native 360 Panorama Person Remover in Rust.
//! Built with Tao + Wry + Rayon. Zero AI, 100% authentic computer vision.

mod processor;
mod xmp;

use base64::prelude::*;
use image::codecs::jpeg::JpegEncoder;
use image::RgbImage;
use processor::ClusterAngles;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

#[derive(Default)]
struct AppState {
    path1: Option<PathBuf>,
    path2: Option<PathBuf>,
    img1_preview: Option<RgbImage>,
    img2_preview: Option<RgbImage>,
    cached_angles: Option<ClusterAngles>,
    last_swap: bool,
    last_threshold: i32,
    last_dilate: i32,
    last_feather: i32,
}

#[derive(Deserialize)]
struct IpcRequest {
    action: String,
    #[serde(default)]
    threshold: Option<i32>,
    #[serde(default)]
    dilate: Option<i32>,
    #[serde(default)]
    feather: Option<i32>,
    #[serde(default)]
    swap: Option<bool>,
}

#[derive(Serialize)]
struct FileLoadedMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    slot: u8,
    name: String,
}

#[derive(Serialize)]
struct PreviewReadyMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    result_b64: String,
    mask_b64: String,
    p1_b64: String,
    p2_b64: String,
    angle_a: f32,
    angle_b: f32,
}

#[derive(Serialize)]
struct StatusMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct SaveSuccessMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    path: String,
}

#[derive(Serialize)]
struct ErrorMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    message: String,
}

fn encode_rgb_to_jpeg_b64(img: &RgbImage, quality: u8) -> String {
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    let mut enc = JpegEncoder::new_with_quality(&mut cursor, quality);
    let _ = enc.encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8);
    BASE64_STANDARD.encode(&buf)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoopBuilder::new().build();
    let window = WindowBuilder::new()
        .with_title("360Cleaner — Удаление человека с 360° панорам (Rust)")
        .with_inner_size(tao::dpi::LogicalSize::new(1220.0, 840.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(960.0, 680.0))
        .build(&event_loop)?;

    let app_state = Arc::new(Mutex::new(AppState::default()));
    let state_for_ipc = Arc::clone(&app_state);

    let html_content = include_str!("ui/index.html");

    let webview = WebViewBuilder::new()
        .with_html(html_content)
        .with_ipc_handler(move |msg| {
            let body = msg.body();
            let req: IpcRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Invalid IPC payload: {}", e);
                    return;
                }
            };

            let state_clone = Arc::clone(&state_for_ipc);

            match req.action.as_str() {
                "pick_file_1" => {
                    if let Some(file) = rfd::FileDialog::new()
                        .set_title("Выберите первое 360° фото")
                        .add_filter("360 Images", &["jpg", "jpeg", "png", "tif", "tiff"])
                        .pick_file()
                    {
                        handle_load_file(1, file, state_clone);
                    }
                }
                "pick_file_2" => {
                    if let Some(file) = rfd::FileDialog::new()
                        .set_title("Выберите второе 360° фото")
                        .add_filter("360 Images", &["jpg", "jpeg", "png", "tif", "tiff"])
                        .pick_file()
                    {
                        handle_load_file(2, file, state_clone);
                    }
                }
                "compute" => {
                    let thresh = req.threshold.unwrap_or(24);
                    let dil = req.dilate.unwrap_or(25);
                    let fea = req.feather.unwrap_or(20);
                    let swp = req.swap.unwrap_or(false);
                    handle_compute(thresh, dil, fea, swp, state_clone);
                }
                "save_file" => {
                    handle_save_export(state_clone);
                }
                _ => {}
            }
        })
        .build(&window)?;

    let webview_arc = Arc::new(webview);
    let webview_for_state = Arc::clone(&webview_arc);

    // Save webview sender into global thread hook or helper
    GLOBAL_WEBVIEW.with(|gw| {
        *gw.borrow_mut() = Some(webview_for_state);
    });

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::DroppedFile(path) => {
                    let st = app_state.lock().unwrap();
                    if st.path1.is_none() {
                        drop(st);
                        handle_load_file(1, path, Arc::clone(&app_state));
                    } else {
                        drop(st);
                        handle_load_file(2, path, Arc::clone(&app_state));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    });
}

thread_local! {
    static GLOBAL_WEBVIEW: std::cell::RefCell<Option<Arc<wry::WebView>>> = const { std::cell::RefCell::new(None) };
}

fn emit_to_webview<T: Serialize>(data: &T) {
    if let Ok(json_str) = serde_json::to_string(data) {
        let script = format!("if (window.onRustMessage) window.onRustMessage({});", json_str);
        GLOBAL_WEBVIEW.with(|gw| {
            if let Some(ref wv) = *gw.borrow() {
                let _ = wv.evaluate_script(&script);
            }
        });
    }
}

fn handle_load_file(slot: u8, path: PathBuf, state_arc: Arc<Mutex<AppState>>) {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

    std::thread::spawn(move || {
        emit_to_webview(&StatusMsg {
            msg_type: "processing_started",
            text: &format!("Загрузка фото {}...", slot),
        });

        match processor::load_image(&path, Some(1400)) {
            Ok(preview) => {
                let mut st = state_arc.lock().unwrap();
                if slot == 1 {
                    st.path1 = Some(path);
                    st.img1_preview = Some(preview);
                } else {
                    st.path2 = Some(path);
                    st.img2_preview = Some(preview);
                }

                emit_to_webview(&FileLoadedMsg {
                    msg_type: "file_loaded",
                    slot,
                    name,
                });

                let both_loaded = st.path1.is_some() && st.path2.is_some();
                let thresh = if st.last_threshold > 0 { st.last_threshold } else { 24 };
                let dil = if st.last_dilate > 0 { st.last_dilate } else { 25 };
                let fea = if st.last_feather > 0 { st.last_feather } else { 20 };
                let swp = st.last_swap;
                drop(st);

                if both_loaded {
                    handle_compute(thresh, dil, fea, swp, state_arc);
                } else {
                    emit_to_webview(&StatusMsg {
                        msg_type: "status",
                        text: "Загрузите второе фото для начала обработки",
                    });
                }
            }
            Err(e) => {
                emit_to_webview(&ErrorMsg {
                    msg_type: "error",
                    message: format!("Не удалось открыть фото: {}", e),
                });
            }
        }
    });
}

fn handle_compute(
    threshold: i32,
    dilate: i32,
    feather: i32,
    swap: bool,
    state_arc: Arc<Mutex<AppState>>,
) {
    std::thread::spawn(move || {
        let (p1_opt, p2_opt) = {
            let mut st = state_arc.lock().unwrap();
            st.last_threshold = threshold;
            st.last_dilate = dilate;
            st.last_feather = feather;
            st.last_swap = swap;
            (st.img1_preview.clone(), st.img2_preview.clone())
        };

        let (img1, mut img2) = match (p1_opt, p2_opt) {
            (Some(i1), Some(i2)) => (i1, i2),
            _ => return,
        };

        emit_to_webview(&StatusMsg {
            msg_type: "processing_started",
            text: "Сведение панорам и удаление человека...",
        });

        // Ensure dimensions match
        if img1.dimensions() != img2.dimensions() {
            let (w, h) = img1.dimensions();
            img2 = image::imageops::resize(&img2, w, h, image::imageops::FilterType::Triangle);
        }

        let (w, h) = img1.dimensions();
        let diff_map = processor::compute_difference_map(&img1, &img2);
        let mut binary_mask: Vec<bool> = diff_map.iter().map(|&d| d > (threshold as f32)).collect();

        // Morphological dilation with circular 360 wrapping
        binary_mask = processor::dilate_mask_360(&binary_mask, w, h, dilate);

        // Find canonical angular cluster centers
        let angles = processor::find_cluster_angles(&binary_mask, w, h);

        // Save computed angles to state so full-res export uses the exact same geometry!
        {
            let mut st = state_arc.lock().unwrap();
            st.cached_angles = Some(angles);
        }

        // Blend preview
        let clean_preview = processor::blend_panoramas(&img1, &img2, &binary_mask, angles, swap, feather);
        let mask_vis = processor::create_mask_visualization(&binary_mask, w, h, angles);

        // Encode to base64 JPEG
        let result_b64 = encode_rgb_to_jpeg_b64(&clean_preview, 92);
        let mask_b64 = encode_rgb_to_jpeg_b64(&mask_vis, 90);
        let p1_b64 = encode_rgb_to_jpeg_b64(&img1, 88);
        let p2_b64 = encode_rgb_to_jpeg_b64(&img2, 88);

        emit_to_webview(&PreviewReadyMsg {
            msg_type: "preview_ready",
            result_b64,
            mask_b64,
            p1_b64,
            p2_b64,
            angle_a: angles.angle_a.to_degrees(),
            angle_b: angles.angle_b.to_degrees(),
        });
    });
}

fn handle_save_export(state_arc: Arc<Mutex<AppState>>) {
    let (p1_path, p2_path, angles_opt, swap, thresh, dil, fea) = {
        let st = state_arc.lock().unwrap();
        (
            st.path1.clone(),
            st.path2.clone(),
            st.cached_angles,
            st.last_swap,
            st.last_threshold,
            st.last_dilate,
            st.last_feather,
        )
    };

    let (path1, path2) = match (p1_path, p2_path) {
        (Some(p1), Some(p2)) => (p1, p2),
        _ => {
            emit_to_webview(&ErrorMsg {
                msg_type: "error",
                message: "Сначала загрузите два 360° фото.".to_string(),
            });
            return;
        }
    };

    let out_file = match rfd::FileDialog::new()
        .set_title("Сохранить чистое 360° фото")
        .set_file_name("clean_360_panorama.jpg")
        .add_filter("JPEG 360 Image", &["jpg", "jpeg"])
        .save_file()
    {
        Some(f) => f,
        None => return,
    };

    std::thread::spawn(move || {
        emit_to_webview(&StatusMsg {
            msg_type: "processing_started",
            text: "Рендеринг финального 360° фото в полном разрешении...",
        });

        // Load full-resolution images
        let img1 = match processor::load_image(&path1, None) {
            Ok(img) => img,
            Err(e) => {
                emit_to_webview(&ErrorMsg {
                    msg_type: "error",
                    message: format!("Ошибка загрузки первого фото в полном разрешении: {}", e),
                });
                return;
            }
        };

        let mut img2 = match processor::load_image(&path2, None) {
            Ok(img) => img,
            Err(e) => {
                emit_to_webview(&ErrorMsg {
                    msg_type: "error",
                    message: format!("Ошибка загрузки второго фото в полном разрешении: {}", e),
                });
                return;
            }
        };

        if img1.dimensions() != img2.dimensions() {
            let (w, h) = img1.dimensions();
            img2 = image::imageops::resize(&img2, w, h, image::imageops::FilterType::Triangle);
        }

        let (w, h) = img1.dimensions();

        // Scale dilate and feather proportionally to resolution (base preview width: 1400)
        let scale = (w as f32 / 1400.0).max(1.0);
        let dil_scaled = ((dil as f32) * scale).round() as i32;
        let fea_scaled = ((fea as f32) * scale).round() as i32;

        let diff_map = processor::compute_difference_map(&img1, &img2);
        let mut binary_mask: Vec<bool> = diff_map.iter().map(|&d| d > (thresh as f32)).collect();
        binary_mask = processor::dilate_mask_360(&binary_mask, w, h, dil_scaled);

        // CRITICAL BUG FIX:
        // Use the EXACT SAME cluster angles that produced the clean preview!
        // Because angles are in spherical coordinates [0, 2*PI), assigning pixels
        // by angular distance is 100% resolution invariant.
        let angles = angles_opt.unwrap_or_else(|| processor::find_cluster_angles(&binary_mask, w, h));

        // Blend full-resolution image
        let clean_full = processor::blend_panoramas(&img1, &img2, &binary_mask, angles, swap, fea_scaled);

        // Save JPEG
        if let Err(e) = clean_full.save(&out_file) {
            emit_to_webview(&ErrorMsg {
                msg_type: "error",
                message: format!("Не удалось сохранить файл: {}", e),
            });
            return;
        }

        // Copy and inject 360 Google Photosphere / GPano XMP & EXIF metadata
        if let Ok(markers) = xmp::extract_jpeg_markers(&path1) {
            let _ = xmp::inject_markers_to_jpeg(&out_file, &markers);
        }

        emit_to_webview(&SaveSuccessMsg {
            msg_type: "save_success",
            path: out_file.to_string_lossy().to_string(),
        });
    });
}

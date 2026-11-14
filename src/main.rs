//! 360Cleaner — Fast, Native 360 Panorama Person Remover in Rust.
//! Tao + Wry + Rayon. Zero AI, 100% classical computer vision.

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
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

/// User events routed back to the main event loop for main-thread operations.
#[derive(Debug)]
enum UserEvent {
    PickFile { slot: u8 },
    FilesDropped { paths: Vec<PathBuf> },
    SaveFile,
}

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
struct FileLoadedMsg {
    #[serde(rename = "type")]
    msg_type: String,
    slot: u8,
    name: String,
    thumb_b64: String,
}

#[derive(Serialize)]
struct PreviewReadyMsg {
    #[serde(rename = "type")]
    msg_type: String,
    result_b64: String,
    mask_b64: String,
    p1_b64: String,
    p2_b64: String,
    angle_a: f32,
    angle_b: f32,
}

#[derive(Serialize)]
struct StatusMsg {
    #[serde(rename = "type")]
    msg_type: String,
    text: String,
}

#[derive(Serialize)]
struct SaveSuccessMsg {
    #[serde(rename = "type")]
    msg_type: String,
    path: String,
}

#[derive(Serialize)]
struct ErrorMsg {
    #[serde(rename = "type")]
    msg_type: String,
    message: String,
}

fn encode_rgb_to_jpeg_b64(img: &RgbImage, quality: u8) -> String {
    let mut buf = Vec::new();
    {
        let mut cursor = Cursor::new(&mut buf);
        let mut enc = JpegEncoder::new_with_quality(&mut cursor, quality);
        let _ = enc.encode(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        );
    }
    BASE64_STANDARD.encode(&buf)
}

fn make_thumbnail(img: &RgbImage) -> String {
    // Small 320px thumbnail for slot previews
    let (w, h) = img.dimensions();
    let thumb = if w > 320 || h > 180 {
        let scale = (320.0 / w as f32).min(180.0 / h as f32);
        let nw = (w as f32 * scale).round() as u32;
        let nh = (h as f32 * scale).round() as u32;
        image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    encode_rgb_to_jpeg_b64(&thumb, 85)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();
    let proxy_for_dd = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("360Cleaner")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 860.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(1000.0, 700.0))
        .build(&event_loop)?;

    let app_state: Arc<Mutex<AppState>> = Arc::new(Mutex::new(AppState::default()));
    let state_for_ipc = Arc::clone(&app_state);

    let html_content = include_str!("ui/index.html");

    let webview = WebViewBuilder::new()
        .with_html(html_content)
        // Intercept file drops BEFORE the WebView can open them as full-window files
        .with_drag_drop_handler(move |event| {
            use wry::DragDropEvent;
            match event {
                DragDropEvent::Drop { paths, .. } => {
                    let _ = proxy_for_dd.send_event(UserEvent::FilesDropped {
                        paths,
                    });
                    true // Prevent WebView default handling (opens file in browser)
                }
                _ => true, // Also block DragEnter/DragOver visual default
            }
        })
        .with_ipc_handler(move |msg| {
            let body = msg.body();
            let req: IpcRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Invalid IPC: {}", e);
                    return;
                }
            };
            let state_clone = Arc::clone(&state_for_ipc);
            let proxy_clone = proxy.clone();

            match req.action.as_str() {
                "pick_file_1" => {
                    // Must open dialog on main thread on macOS — route via EventLoop proxy
                    let _ = proxy_clone.send_event(UserEvent::PickFile { slot: 1 });
                }
                "pick_file_2" => {
                    let _ = proxy_clone.send_event(UserEvent::PickFile { slot: 2 });
                }
                "compute" => {
                    let thresh = req.threshold.unwrap_or(24);
                    let dil = req.dilate.unwrap_or(25);
                    let fea = req.feather.unwrap_or(20);
                    let swp = req.swap.unwrap_or(false);
                    handle_compute(thresh, dil, fea, swp, state_clone);
                }
                "save_file" => {
                    let _ = proxy_clone.send_event(UserEvent::SaveFile);
                }
                _ => {}
            }
        })
        .build(&window)?;

    let webview_arc = Arc::new(webview);
    let webview_for_state = Arc::clone(&webview_arc);

    GLOBAL_WEBVIEW.with(|gw| {
        *gw.borrow_mut() = Some(webview_for_state);
    });

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                _ => {}
            },

            Event::UserEvent(user_event) => match user_event {
                // File picker runs on main thread — works on macOS and Windows
                UserEvent::PickFile { slot } => {
                    let title = if slot == 1 {
                        "Выберите первое 360° фото"
                    } else {
                        "Выберите второе 360° фото"
                    };
                    if let Some(file) = rfd::FileDialog::new()
                        .set_title(title)
                        .add_filter("360 Images", &["jpg", "jpeg", "png", "tif", "tiff"])
                        .pick_file()
                    {
                        handle_load_file(slot, file, Arc::clone(&app_state));
                    }
                }

                // Handle dropped files — support 1 or 2 files at once
                UserEvent::FilesDropped { paths } => {
                    let image_paths: Vec<PathBuf> = paths
                        .into_iter()
                        .filter(|p| {
                            matches!(
                                p.extension()
                                    .and_then(|e| e.to_str())
                                    .map(|e| e.to_lowercase())
                                    .as_deref(),
                                Some("jpg" | "jpeg" | "png" | "tif" | "tiff")
                            )
                        })
                        .collect();

                    if image_paths.is_empty() {
                        return;
                    }

                    let st = app_state.lock().unwrap();
                    let has1 = st.path1.is_some();
                    let has2 = st.path2.is_some();
                    drop(st);

                    if image_paths.len() >= 2 {
                        // Two photos dropped at once → slot 1 and slot 2
                        handle_load_file(1, image_paths[0].clone(), Arc::clone(&app_state));
                        handle_load_file(2, image_paths[1].clone(), Arc::clone(&app_state));
                    } else {
                        // Single photo: fill the first empty slot
                        if !has1 {
                            handle_load_file(1, image_paths[0].clone(), Arc::clone(&app_state));
                        } else if !has2 {
                            handle_load_file(2, image_paths[0].clone(), Arc::clone(&app_state));
                        } else {
                            // Both slots taken — replace slot 1 and re-compute
                            handle_load_file(1, image_paths[0].clone(), Arc::clone(&app_state));
                        }
                    }
                }

                UserEvent::SaveFile => {
                    handle_save_export(Arc::clone(&app_state));
                }
            },

            _ => {}
        }
    });
}

thread_local! {
    static GLOBAL_WEBVIEW: std::cell::RefCell<Option<Arc<wry::WebView>>> =
        const { std::cell::RefCell::new(None) };
}

fn emit_to_webview<T: Serialize>(data: &T) {
    if let Ok(json_str) = serde_json::to_string(data) {
        let script = format!("if(window.onRustMessage)window.onRustMessage({});", json_str);
        GLOBAL_WEBVIEW.with(|gw| {
            if let Some(ref wv) = *gw.borrow() {
                let _ = wv.evaluate_script(&script);
            }
        });
    }
}

fn handle_load_file(slot: u8, path: PathBuf, state_arc: Arc<Mutex<AppState>>) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    std::thread::spawn(move || {
        emit_to_webview(&StatusMsg {
            msg_type: "loading_file".into(),
            text: format!("Загрузка фото {} — {}…", slot, name),
        });

        match processor::load_image(&path, Some(1400)) {
            Ok(preview) => {
                let thumb_b64 = make_thumbnail(&preview);

                let mut st = state_arc.lock().unwrap();
                if slot == 1 {
                    st.path1 = Some(path);
                    st.img1_preview = Some(preview);
                } else {
                    st.path2 = Some(path);
                    st.img2_preview = Some(preview);
                }

                emit_to_webview(&FileLoadedMsg {
                    msg_type: "file_loaded".into(),
                    slot,
                    name,
                    thumb_b64,
                });

                let both_loaded = st.path1.is_some() && st.path2.is_some();
                let thresh = if st.last_threshold > 0 { st.last_threshold } else { 24 };
                let dil = if st.last_dilate > 0 { st.last_dilate } else { 25 };
                let fea = if st.last_feather > 0 { st.last_feather } else { 20 };
                let swp = st.last_swap;
                drop(st);

                if both_loaded {
                    handle_compute(thresh, dil, fea, swp, state_arc);
                }
            }
            Err(e) => {
                emit_to_webview(&ErrorMsg {
                    msg_type: "error".into(),
                    message: format!("Не удалось открыть: {}", e),
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
            msg_type: "processing_started".into(),
            text: "Анализ различий и удаление человека…".into(),
        });

        if img1.dimensions() != img2.dimensions() {
            let (w, h) = img1.dimensions();
            img2 = image::imageops::resize(&img2, w, h, image::imageops::FilterType::Triangle);
        }

        let (w, h) = img1.dimensions();
        let diff_map = processor::compute_difference_map(&img1, &img2);
        let mut binary_mask: Vec<bool> =
            diff_map.iter().map(|&d| d > (threshold as f32)).collect();
        binary_mask = processor::dilate_mask_360(&binary_mask, w, h, dilate);

        let angles = processor::find_cluster_angles(&binary_mask, w, h);

        {
            let mut st = state_arc.lock().unwrap();
            st.cached_angles = Some(angles);
        }

        let clean_preview =
            processor::blend_panoramas(&img1, &img2, &binary_mask, angles, swap, feather);
        let mask_vis = processor::create_mask_visualization(&binary_mask, w, h, angles);

        let result_b64 = encode_rgb_to_jpeg_b64(&clean_preview, 92);
        let mask_b64 = encode_rgb_to_jpeg_b64(&mask_vis, 90);
        let p1_b64 = encode_rgb_to_jpeg_b64(&img1, 85);
        let p2_b64 = encode_rgb_to_jpeg_b64(&img2, 85);

        emit_to_webview(&PreviewReadyMsg {
            msg_type: "preview_ready".into(),
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
                msg_type: "error".into(),
                message: "Загрузите оба фото.".into(),
            });
            return;
        }
    };

    let out_file = match rfd::FileDialog::new()
        .set_title("Сохранить чистое 360° фото")
        .set_file_name("clean_360_panorama.jpg")
        .add_filter("JPEG Image", &["jpg", "jpeg"])
        .save_file()
    {
        Some(f) => f,
        None => return,
    };

    std::thread::spawn(move || {
        emit_to_webview(&StatusMsg {
            msg_type: "processing_started".into(),
            text: "Финальный рендеринг в полном разрешении…".into(),
        });

        let img1 = match processor::load_image(&path1, None) {
            Ok(img) => img,
            Err(e) => {
                emit_to_webview(&ErrorMsg {
                    msg_type: "error".into(),
                    message: format!("Ошибка: {}", e),
                });
                return;
            }
        };

        let mut img2 = match processor::load_image(&path2, None) {
            Ok(img) => img,
            Err(e) => {
                emit_to_webview(&ErrorMsg {
                    msg_type: "error".into(),
                    message: format!("Ошибка: {}", e),
                });
                return;
            }
        };

        if img1.dimensions() != img2.dimensions() {
            let (w, h) = img1.dimensions();
            img2 = image::imageops::resize(&img2, w, h, image::imageops::FilterType::Triangle);
        }

        let (w, h) = img1.dimensions();
        let scale = (w as f32 / 1400.0).max(1.0);
        let dil_scaled = ((dil as f32) * scale).round() as i32;
        let fea_scaled = ((fea as f32) * scale).round() as i32;

        let diff_map = processor::compute_difference_map(&img1, &img2);
        let mut binary_mask: Vec<bool> =
            diff_map.iter().map(|&d| d > (thresh as f32)).collect();
        binary_mask = processor::dilate_mask_360(&binary_mask, w, h, dil_scaled);

        let angles =
            angles_opt.unwrap_or_else(|| processor::find_cluster_angles(&binary_mask, w, h));

        let clean_full =
            processor::blend_panoramas(&img1, &img2, &binary_mask, angles, swap, fea_scaled);

        if let Err(e) = clean_full.save(&out_file) {
            emit_to_webview(&ErrorMsg {
                msg_type: "error".into(),
                message: format!("Не удалось сохранить: {}", e),
            });
            return;
        }

        if let Ok(markers) = xmp::extract_jpeg_markers(&path1) {
            let _ = xmp::inject_markers_to_jpeg(&out_file, &markers);
        }

        emit_to_webview(&SaveSuccessMsg {
            msg_type: "save_success".into(),
            path: out_file.to_string_lossy().to_string(),
        });
    });
}

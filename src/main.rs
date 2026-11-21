//! 360Cleaner — убирает фотографа и людей с 360° панорамы по кадрам с одной точки.
//! Одно фото — только пересохранение (размер, качество), два — фотограф, три и больше — прохожие.
//! Окно: tao + wry (системный WebView), обработка: Rayon. Без нейросетей.
//!
//! Потоки. WebView живёт только в главном потоке, поэтому всё, что идёт в окно, отправляется событием
//! `UserEvent::Js` через `EventLoopProxy` и выполняется там. Диалоги выбора файла тоже открываются в главном потоке
//! (на Mac иначе нельзя). Расчёт превью — один рабочий поток: из очереди берётся только последний запрос, а результат
//! устаревшего расчёта (фото или ползунки успели поменяться) не показывается.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cubemap;
mod donations;
mod export;
mod i18n;
mod net;
mod processor;
mod settings;
mod update;
mod xmp;

use base64::prelude::*;
use image::RgbImage;
use image::codecs::jpeg::JpegEncoder;
use processor::{Alpha, Params};
use serde::Deserialize;
use serde_json::{Value, json};
use settings::Settings;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use update::{Release, format_version};
use wry::{DragDropEvent, WebViewBuilder};

const IMAGE_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "tif", "tiff"];
/// Больше кадров расчёт не ускорит и не улучшит, а памяти на превью уйдёт много.
const MAX_PHOTOS: usize = 16;
/// Сколько фото открывается одновременно: каждое в полном размере занимает сотни мегабайт.
const LOADERS: usize = 2;

#[derive(Debug)]
enum UserEvent {
    /// Скрипт для WebView (выполняется в главном потоке).
    Js(String),
    PickFiles,
    FilesDropped(Vec<PathBuf>),
    Save,
    Quit,
}

/// Отправка сообщений в окно из любого потока.
#[derive(Clone)]
struct Ui(EventLoopProxy<UserEvent>);

impl Ui {
    fn send(&self, msg: Value) {
        let script = format!("window.onRustMessage&&window.onRustMessage({msg});");
        let _ = self.0.send_event(UserEvent::Js(script));
    }

    // Тексты переводит окно: сюда идут ключ словаря `I18N` и параметры подстановки.

    fn status(&self, key: &str, args: Value, busy: bool) {
        self.send(json!({"type": "status", "key": key, "args": args, "busy": busy}));
    }

    fn error(&self, key: &str, args: Value) {
        self.send(json!({"type": "error", "key": key, "args": args}));
    }

    fn processing(&self, key: &str) {
        self.send(json!({"type": "processing", "key": key}));
    }
}

struct Photo {
    id: u64,
    path: PathBuf,
    /// Размер файла в пикселях.
    dims: Option<(u32, u32)>,
    /// None — ещё открывается.
    preview: Option<RgbImage>,
}

/// Результат расчёта, по которому сохраняется файл.
struct Computed {
    generation: u64,
    /// Пути кадров по порядку и маски кадров 1.. (у одного фото масок нет).
    paths: Vec<PathBuf>,
    alphas: Vec<Alpha>,
}

#[derive(Default)]
struct UpdateState {
    checking: bool,
    manual: bool,
    downloading: bool,
    available: Option<Release>,
}

struct State {
    photos: Vec<Photo>,
    next_id: u64,
    params: Params,
    /// Растёт при каждой смене фото или параметров.
    generation: u64,
    computed: Option<Computed>,
    exporting: bool,
    last_saved: Option<PathBuf>,
    settings: Settings,
    /// Язык интерфейса (код из `i18n::LANGUAGES`).
    lang: &'static str,
    /// Файл донатов целиком: разбирается на языке интерфейса при показе.
    donations: Vec<u8>,
    donations_loading: bool,
    updates: UpdateState,
    /// Запрос пересчитать примерный вес файла (`estimate_worker`).
    estimate: Sender<()>,
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Deserialize)]
struct Request {
    action: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    size: Option<u32>,
    #[serde(default)]
    quality: Option<u8>,
    #[serde(default)]
    format: Option<export::Format>,
    #[serde(default)]
    threshold: Option<f32>,
    #[serde(default)]
    dilate: Option<i32>,
    #[serde(default)]
    feather: Option<i32>,
    #[serde(default)]
    swap: Option<bool>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    value: Option<bool>,
    #[serde(default)]
    lang: Option<String>,
}

fn jpeg_bytes(img: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality).encode_image(img).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn jpeg_b64(img: &RgbImage, quality: u8) -> String {
    jpeg_bytes(img, quality).map(|b| BASE64_STANDARD.encode(b)).unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Страница окна со словарём переводов внутри.
fn page() -> String {
    include_str!("ui/index.html").replace("/*I18N*/", include_str!("ui/i18n.js"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let ui = Ui(event_loop.create_proxy());

    let settings = Settings::load();
    let params = Params {
        threshold: settings.threshold.clamp(5.0, 60.0),
        dilate: settings.dilate.clamp(5, 60),
        feather: settings.feather.clamp(2, 50),
        swap: false,
    };
    let (estimate_tx, estimate_rx) = channel::<()>();
    let state: Shared = Arc::new(Mutex::new(State {
        photos: Vec::new(),
        next_id: 1,
        params,
        generation: 0,
        computed: None,
        exporting: false,
        last_saved: None,
        lang: i18n::resolve(&settings.language),
        settings,
        donations: donations::cached(),
        donations_loading: false,
        updates: UpdateState::default(),
        estimate: estimate_tx,
    }));

    let (compute_tx, compute_rx) = channel::<()>();
    {
        let (state, ui) = (Arc::clone(&state), ui.clone());
        std::thread::spawn(move || compute_worker(compute_rx, state, ui));
    }
    {
        let (state, ui) = (Arc::clone(&state), ui.clone());
        std::thread::spawn(move || estimate_worker(estimate_rx, state, ui));
    }
    let loader = start_loaders(&state, &ui, &compute_tx);

    let window = WindowBuilder::new()
        .with_title("360Cleaner")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 860.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(960.0, 640.0))
        .build(&event_loop)?;

    let drop_ui = ui.clone();
    let drop_proxy = event_loop.create_proxy();
    let ipc_state = Arc::clone(&state);
    let ipc_ui = ui.clone();
    let ipc_proxy = event_loop.create_proxy();
    let ipc_compute = compute_tx.clone();

    let webview = WebViewBuilder::new()
        .with_html(page())
        // файлы из Finder / Проводника перехватываются до WebView, иначе он открыл бы картинку вместо программы
        .with_drag_drop_handler(move |event| {
            match event {
                DragDropEvent::Enter { paths, .. } => {
                    let ok = paths.iter().any(|p| is_image(p));
                    drop_ui.send(json!({"type": "drag", "on": true, "ok": ok}));
                }
                DragDropEvent::Leave => drop_ui.send(json!({"type": "drag", "on": false})),
                DragDropEvent::Drop { paths, .. } => {
                    drop_ui.send(json!({"type": "drag", "on": false}));
                    let _ = drop_proxy.send_event(UserEvent::FilesDropped(paths));
                }
                _ => {}
            }
            true
        })
        .with_ipc_handler(move |msg| match serde_json::from_str::<Request>(msg.body()) {
            Ok(req) => handle_request(req, &ipc_state, &ipc_ui, &ipc_proxy, &ipc_compute),
            Err(e) => eprintln!("bad message from the window: {e}"),
        })
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(user_event) => match user_event {
                UserEvent::Js(script) => {
                    let _ = webview.evaluate_script(&script);
                }
                UserEvent::PickFiles => {
                    let (lang, dir) = {
                        let st = lock(&state);
                        (st.lang, st.photos.last().and_then(|p| p.path.parent().map(Path::to_path_buf)))
                    };
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(i18n::tr(lang, i18n::Text::PickPhotos))
                        .add_filter(i18n::tr(lang, i18n::Text::Photos), &IMAGE_EXTENSIONS);
                    if let Some(dir) = dir {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(files) = dialog.pick_files() {
                        add_files(files, &state, &ui, &loader);
                    }
                }
                UserEvent::FilesDropped(paths) => add_files(paths, &state, &ui, &loader),
                UserEvent::Save => save(&state, &ui),
                UserEvent::Quit => *control_flow = ControlFlow::Exit,
            },
            _ => {}
        }
    });
}

fn handle_request(req: Request, state: &Shared, ui: &Ui, proxy: &EventLoopProxy<UserEvent>, compute: &Sender<()>) {
    match req.action.as_str() {
        "ready" => {
            let st = lock(state);
            let p = st.params;
            ui.send(json!({
                "type": "init",
                "lang": st.lang,
                "language": st.settings.language,
                "languages": i18n::LANGUAGES,
                "systemLang": i18n::system_language(),
                "version": env!("CARGO_PKG_VERSION"),
                "repo": update::REPO,
                "threshold": p.threshold, "dilate": p.dilate, "feather": p.feather,
                "exportWidth": st.settings.export_width, "exportQuality": st.settings.export_quality,
                "exportFormat": st.settings.export_format,
                "exportWidths": export::WIDTHS,
                "colors": processor::SOURCE_COLORS,
                "autoUpdate": st.settings.auto_update,
                "canUpdate": update::install_target().is_some(),
                "updateFailed": std::env::args().any(|a| a == "--update-failed"),
            }));
            let auto = st.settings.auto_update
                && update::install_target().is_some()
                && settings::now().saturating_sub(st.settings.last_update_check) >= 86_400;
            drop(st);
            if auto {
                check_updates(state, ui, false);
            }
            ui.send(photos_msg(&lock(state)));
            let _ = lock(state).estimate.send(());
            // фото из командной строки: `360Cleaner a.jpg b.jpg`
            let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.is_file()).collect();
            if !paths.is_empty() {
                let _ = proxy.send_event(UserEvent::FilesDropped(paths));
            }
        }
        "pick" => {
            let _ = proxy.send_event(UserEvent::PickFiles);
        }
        "remove" => {
            let mut st = lock(state);
            let before = st.photos.len();
            st.photos.retain(|p| Some(p.id) != req.id);
            if st.photos.len() != before {
                photos_changed(&mut st, ui);
                drop(st);
                let _ = compute.send(());
            }
        }
        "clear" => {
            let mut st = lock(state);
            st.photos.clear();
            photos_changed(&mut st, ui);
            ui.status("stReady", json!({}), false);
        }
        "export_options" => {
            let mut st = lock(state);
            set_export_options(&mut st, &req);
        }
        "params" => {
            let mut st = lock(state);
            let old = st.params;
            let p = Params {
                threshold: req.threshold.unwrap_or(old.threshold).clamp(1.0, 100.0),
                dilate: req.dilate.unwrap_or(old.dilate).clamp(0, 100),
                feather: req.feather.unwrap_or(old.feather).clamp(0, 100),
                swap: req.swap.unwrap_or(old.swap),
            };
            if p == old {
                return;
            }
            st.params = p;
            st.generation += 1;
            st.settings.threshold = p.threshold;
            st.settings.dilate = p.dilate;
            st.settings.feather = p.feather;
            st.settings.save();
            drop(st);
            let _ = compute.send(());
        }
        "save" => {
            set_export_options(&mut lock(state), &req);
            let _ = proxy.send_event(UserEvent::Save);
        }
        "reveal" => {
            if let Some(path) = lock(state).last_saved.clone()
                && let Err(e) = net::reveal(&path)
            {
                ui.error("errReveal", json!({"err": e.to_string()}));
            }
        }
        "quit" => {
            let _ = proxy.send_event(UserEvent::Quit);
        }
        "donate_open" => open_donations(state, ui),
        "open_url" => {
            if let Some(url) = req.url
                && let Err(e) = net::open_url(&url)
            {
                ui.error("errBrowser", json!({"err": e.to_string()}));
            }
        }
        "update_check" => check_updates(state, ui, true),
        "update_install" => install_update(state, ui, proxy),
        "update_skip" => {
            let mut st = lock(state);
            if let Some(r) = &st.updates.available {
                st.settings.skipped_version = format_version(r.version);
                st.settings.save();
            }
        }
        "update_page" => {
            let url = lock(state).updates.available.as_ref().map(|r| r.page_url.clone());
            let url = url
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| format!("https://github.com/{}/releases", update::REPO));
            let _ = net::open_url(&url);
        }
        "set_language" => {
            let mut st = lock(state);
            let setting = req.lang.unwrap_or_default();
            st.settings.language = i18n::supported(&setting).map(str::to_string).unwrap_or_default();
            st.lang = i18n::resolve(&st.settings.language);
            st.settings.save();
            ui.send(json!({"type": "lang", "lang": st.lang, "language": st.settings.language}));
        }
        "set_auto_update" => {
            let mut st = lock(state);
            st.settings.auto_update = req.value.unwrap_or(true);
            st.settings.save();
        }
        other => eprintln!("unknown action from the window: {other}"),
    }
}

fn set_export_options(st: &mut State, req: &Request) {
    let s = &mut st.settings;
    let old = (s.export_width, s.export_quality, s.export_format);
    if let Some(w) = req.size {
        s.export_width = if export::WIDTHS.contains(&w) { w } else { 0 };
    }
    if let Some(q) = req.quality {
        s.export_quality = q.clamp(settings::MIN_QUALITY, 100);
    }
    if let Some(f) = req.format {
        s.export_format = f;
    }
    if (s.export_width, s.export_quality, s.export_format) != old {
        s.save();
    }
    if (s.export_width, s.export_format) != (old.0, old.2) {
        let _ = st.estimate.send(());
    }
}

// ---------------------------------------------------------------- фото

/// Список фото для окна: порядок, имена, размеры; картинки приходят отдельно (`photo_loaded`).
fn photos_msg(st: &State) -> Value {
    let list: Vec<Value> = st
        .photos
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": file_name(&p.path),
                "size": p.dims.map(|(w, h)| format!("{w}×{h}")),
                "width": p.dims.map(|d| d.0),
                "loading": p.preview.is_none(),
            })
        })
        .collect();
    json!({"type": "photos", "list": list})
}

/// После любой смены списка: старый расчёт больше не годится.
fn photos_changed(st: &mut State, ui: &Ui) {
    st.generation += 1;
    st.computed = None;
    ui.send(photos_msg(st));
    let _ = st.estimate.send(());
}

/// Новые фото — в конец списка; уже добавленные пропускаются. Расчёт запустится, когда откроется последнее.
fn add_files(paths: Vec<PathBuf>, state: &Shared, ui: &Ui, loader: &SyncSender<(u64, PathBuf)>) {
    let images: Vec<PathBuf> = paths.into_iter().filter(|p| is_image(p)).collect();
    if images.is_empty() {
        ui.error("errDropTypes", json!({}));
        return;
    }
    let mut st = lock(state);
    let mut added = Vec::new();
    for path in images {
        if st.photos.iter().any(|p| p.path == path) {
            continue;
        }
        if st.photos.len() >= MAX_PHOTOS {
            ui.error("errTooMany", json!({"n": MAX_PHOTOS}));
            break;
        }
        let id = st.next_id;
        st.next_id += 1;
        st.photos.push(Photo { id, path: path.clone(), dims: None, preview: None });
        added.push((id, path));
    }
    if added.is_empty() {
        return;
    }
    photos_changed(&mut st, ui);
    drop(st);
    for job in added {
        if loader.send(job).is_err() {
            break;
        }
    }
}

/// Потоки, открывающие фото по очереди (`LOADERS` одновременно).
fn start_loaders(state: &Shared, ui: &Ui, compute: &Sender<()>) -> SyncSender<(u64, PathBuf)> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<(u64, PathBuf)>(MAX_PHOTOS * 4);
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..LOADERS {
        let (rx, state, ui, compute) = (Arc::clone(&rx), Arc::clone(state), ui.clone(), compute.clone());
        std::thread::spawn(move || {
            loop {
                let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                let Ok((id, path)) = job else { break };
                load_photo(id, path, &state, &ui, &compute);
            }
        });
    }
    tx
}

fn load_photo(id: u64, path: PathBuf, state: &Shared, ui: &Ui, compute: &Sender<()>) {
    if !lock(state).photos.iter().any(|p| p.id == id) {
        return; // убрали из списка, пока ждало очереди
    }
    let name = file_name(&path);
    ui.status("stLoading", json!({"name": name}), true);
    let dims = image::image_dimensions(&path).ok();
    match processor::load_image(&path, Some(processor::PREVIEW_DIM)) {
        Ok(preview) => {
            let image_b64 = jpeg_b64(&preview, 85);
            let mut st = lock(state);
            let first = st.photos.first().and_then(|p| p.dims);
            let Some(photo) = st.photos.iter_mut().find(|p| p.id == id) else { return };
            photo.preview = Some(preview);
            photo.dims = dims;
            let mut warning = None;
            if let Some((w, h)) = dims
                && (w as f32 / h as f32 - 2.0).abs() > 0.05
            {
                warning = Some(("warnNot360", json!({"name": name, "w": w, "h": h})));
            } else if first.is_some() && dims.is_some() && first != dims && st.photos[0].id != id {
                warning = Some(("warnSizes", json!({"name": name})));
            }
            ui.send(json!({"type": "photo_loaded", "id": id, "image_b64": image_b64}));
            photos_changed(&mut st, ui);
            let waiting = st.photos.iter().any(|p| p.preview.is_none());
            drop(st);
            if let Some((key, args)) = warning {
                ui.send(json!({"type": "warning", "key": key, "args": args}));
            }
            if !waiting {
                let _ = compute.send(());
            }
        }
        Err(e) => {
            let mut st = lock(state);
            st.photos.retain(|p| p.id != id);
            photos_changed(&mut st, ui);
            let waiting = st.photos.iter().any(|p| p.preview.is_none());
            drop(st);
            ui.error("errOpen", json!({"name": name, "err": e}));
            if !waiting {
                let _ = compute.send(());
            }
        }
    }
}

fn compute_worker(rx: Receiver<()>, state: Shared, ui: Ui) {
    while rx.recv().is_ok() {
        // из накопившихся запросов нужен только последний
        while rx.try_recv().is_ok() {}
        let (frames, paths, params, generation) = {
            let st = lock(&state);
            if st.photos.is_empty() || st.photos.iter().any(|p| p.preview.is_none()) {
                continue;
            }
            if st.computed.as_ref().is_some_and(|c| c.generation == st.generation) {
                continue;
            }
            let frames: Vec<RgbImage> = st.photos.iter().filter_map(|p| p.preview.clone()).collect();
            let paths: Vec<PathBuf> = st.photos.iter().map(|p| p.path.clone()).collect();
            (frames, paths, st.params, st.generation)
        };
        let n = frames.len();
        if n > 1 {
            ui.processing("procPreview");
        }
        let (w, h) = frames[0].dimensions();
        let mut frames = frames.into_iter();
        let first = frames.next().unwrap_or_default();
        let others: Vec<RgbImage> = frames.map(|f| processor::match_size(f, w, h)).collect();
        let started = std::time::Instant::now();
        let mut msg = json!({"type": "preview_ready", "count": n});
        let (result, mask, alphas) = match others.len() {
            0 => (first, None, Vec::new()),
            1 => {
                let p = processor::compute_preview(&first, &others[0], params);
                msg["angle_a"] = json!(p.angles.angle_a.to_degrees());
                msg["angle_b"] = json!(p.angles.angle_b.to_degrees());
                msg["changed"] = json!(p.changed);
                msg["too_many"] = json!(p.too_many);
                if p.threshold > params.threshold {
                    msg["threshold_raised"] = json!(p.threshold);
                }
                (p.result, Some(p.mask_vis), vec![p.alpha])
            }
            _ => {
                let mut all = Vec::with_capacity(n);
                all.push(first);
                all.extend(others);
                let p = processor::compute_stack_preview(&all, params);
                msg["changed"] = json!(p.changed);
                (p.result, Some(p.mask_vis), p.alphas)
            }
        };
        msg["ms"] = json!(started.elapsed().as_millis());

        let mut st = lock(&state);
        if st.generation != generation {
            // фото или параметры поменялись во время расчёта: этот результат никому не нужен
            continue;
        }
        st.computed = Some(Computed { generation, paths, alphas });
        drop(st);
        msg["result_b64"] = json!(jpeg_b64(&result, 90));
        msg["mask_b64"] = json!(mask.map(|m| jpeg_b64(&m, 85)));
        ui.send(msg);
    }
}

/// Примерный вес файла при каждом качестве от `MIN_QUALITY` до 100 — для подписи у ползунка качества.
/// Зависит только от первого фото и размера результата; из накопившихся запросов нужен последний.
fn estimate_worker(rx: Receiver<()>, state: Shared, ui: Ui) {
    let qualities: Vec<u8> = (settings::MIN_QUALITY..=100).collect();
    let mut done: Option<(Option<u64>, u32, export::Format)> = None;
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        let (first, width, format) = {
            let st = lock(&state);
            (st.photos.first().map(|p| (p.id, p.path.clone())), st.settings.export_width, st.settings.export_format)
        };
        let key = (first.as_ref().map(|f| f.0), width, format);
        if done == Some(key) {
            continue;
        }
        done = Some(key);
        let sizes = match &first {
            None => Value::Null,
            Some((_, path)) => match export::estimate_sizes(path, width, format, &qualities) {
                Ok(list) => list.into_iter().map(|(q, bytes)| (q.to_string(), json!(bytes))).collect(),
                Err(e) => {
                    eprintln!("size estimate: {e}");
                    Value::Null
                }
            },
        };
        ui.send(json!({"type": "size_estimate", "id": key.0, "width": width, "format": format, "sizes": sizes}));
    }
}

fn save(state: &Shared, ui: &Ui) {
    let st = lock(state);
    if st.exporting {
        return;
    }
    if st.photos.is_empty() {
        ui.error("errNoPhotos", json!({}));
        return;
    }
    let Some(computed) = st.computed.as_ref().filter(|c| c.generation == st.generation) else {
        ui.error("errWaitPreview", json!({}));
        return;
    };
    let (paths, alphas, lang) = (computed.paths.clone(), computed.alphas.clone(), st.lang);
    let s = &st.settings;
    let options = export::Options { width: s.export_width, quality: s.export_quality, format: s.export_format };
    let dims = st.photos[0].dims;
    drop(st);

    let first = &paths[0];
    let resized = dims.is_some_and(|(w, h)| export::target_dims(w, h, options.width) != (w, h));
    let stem = first.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let suffix = match (paths.len() > 1, resized) {
        (true, true) => format!("_clean_{}", options.width),
        (true, false) => "_clean".to_string(),
        (false, true) => format!("_{}", options.width),
        (false, false) if options.format == export::Format::Equirect => format!("_q{}", options.quality),
        (false, false) => String::new(),
    } + options.format.tag();
    let mut dialog = rfd::FileDialog::new()
        .set_title(i18n::tr(lang, i18n::Text::SaveTitle))
        .set_file_name(format!("{stem}{suffix}.jpg"))
        .add_filter("JPEG", &["jpg", "jpeg"]);
    if let Some(dir) = first.parent() {
        dialog = dialog.set_directory(dir);
    }
    let Some(mut out) = dialog.save_file() else { return };
    if !out.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")) {
        out.set_extension("jpg");
    }
    if export::output_paths(&out, options.format).iter().any(|p| paths.contains(p)) {
        ui.error("errOverwrite", json!({}));
        return;
    }
    lock(state).exporting = true;

    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        ui.processing("procSave");
        let result = export::export(&paths, &alphas, options, &out);
        let mut st = lock(&state);
        st.exporting = false;
        match result {
            Ok(saved) => {
                let shown = saved.files.first().cloned().unwrap_or_else(|| out.clone());
                st.last_saved = Some(shown.clone());
                drop(st);
                // шесть граней: «имя_*.jpg»
                let name = if saved.files.len() > 1 {
                    format!("{}_*.jpg", out.file_stem().unwrap_or_default().to_string_lossy())
                } else {
                    file_name(&shown)
                };
                ui.send(json!({
                    "type": "save_success", "path": shown.to_string_lossy(), "name": name,
                    "files": saved.files.len(),
                    "w": saved.width, "h": saved.height, "mb": format!("{:.1}", saved.bytes as f64 / 1_048_576.0),
                }));
            }
            Err(e) => {
                drop(st);
                ui.error("errSave", json!({"err": e}));
            }
        }
    });
}

// ---------------------------------------------------------------- донаты

fn donations_msg(st: &State, loading: bool, offline: bool) -> Value {
    let info = donations::info(&st.donations, st.lang);
    json!({"type": "donations", "info": info, "loading": loading, "offline": offline})
}

/// При каждом открытии окна файл загружается заново; до ответа видна последняя копия.
fn open_donations(state: &Shared, ui: &Ui) {
    let mut st = lock(state);
    ui.send(donations_msg(&st, true, false));
    if st.donations_loading {
        return;
    }
    st.donations_loading = true;
    drop(st);
    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        let fresh = donations::download();
        let mut st = lock(&state);
        st.donations_loading = false;
        let offline = fresh.is_none();
        if let Some(data) = fresh {
            st.donations = data;
        }
        ui.send(donations_msg(&st, false, offline));
    });
}

// ---------------------------------------------------------------- обновления

/// `status` — (вид: ok / err / new, ключ словаря, параметры).
fn update_msg(st: &State, status: Option<(&str, &str, Value)>) -> Value {
    let target = update::install_target();
    let writable = target.as_deref().is_some_and(update::can_write);
    let available = st
        .updates
        .available
        .as_ref()
        .map(|r| json!({"version": format_version(r.version), "notes": r.notes, "page": r.page_url}));
    json!({
        "type": "update",
        "checking": st.updates.checking,
        "downloading": st.updates.downloading,
        "available": available,
        "writable": writable,
        "status": status.map(|(kind, key, args)| json!({"kind": kind, "key": key, "args": args})),
    })
}

fn check_updates(state: &Shared, ui: &Ui, manual: bool) {
    let mut st = lock(state);
    if st.updates.checking || st.updates.downloading {
        return;
    }
    st.updates.checking = true;
    st.updates.manual = manual;
    ui.send(update_msg(&st, None));
    drop(st);
    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        let result = update::latest();
        let mut st = lock(&state);
        st.updates.checking = false;
        let current = update::current();
        let status = match result {
            Ok(found) => {
                st.settings.last_update_check = settings::now();
                st.settings.save();
                match found.filter(|r| r.version > current) {
                    Some(r) => {
                        let v = format_version(r.version);
                        let offer = st.updates.manual || st.settings.skipped_version != v;
                        st.updates.available = Some(r);
                        let msg = update_msg(&st, Some(("new", "updNew", json!({"v": v}))));
                        ui.send(msg);
                        if offer {
                            ui.send(json!({"type": "update_offer"}));
                        }
                        return;
                    }
                    None => {
                        st.updates.available = None;
                        ("ok", "updLatest", json!({"v": format_version(current)}))
                    }
                }
            }
            Err(e) => ("err", "updCheckFailed", json!({"err": e})),
        };
        ui.send(update_msg(&st, Some(status)));
    });
}

fn install_update(state: &Shared, ui: &Ui, proxy: &EventLoopProxy<UserEvent>) {
    let mut st = lock(state);
    if st.updates.downloading {
        return;
    }
    if st.exporting {
        ui.error("errWaitSave", json!({}));
        return;
    }
    let Some(release) = st.updates.available.clone() else { return };
    st.updates.downloading = true;
    ui.send(update_msg(&st, Some(("new", "updDownloading", json!({"v": format_version(release.version)})))));
    drop(st);
    let (state, ui, proxy) = (Arc::clone(state), ui.clone(), proxy.clone());
    std::thread::spawn(move || {
        let result = update::download(&release).and_then(|source| {
            let target = update::install_target().ok_or("could not find where the app is installed")?;
            update::start_install(&source, &target).map_err(|e| format!("could not start the installer: {e}"))
        });
        let mut st = lock(&state);
        st.updates.downloading = false;
        match result {
            Ok(()) => {
                ui.send(update_msg(&st, Some(("ok", "updRestarting", json!({})))));
                let _ = proxy.send_event(UserEvent::Quit);
            }
            Err(e) => ui.send(update_msg(&st, Some(("err", "updFailed", json!({"err": e}))))),
        }
    });
}

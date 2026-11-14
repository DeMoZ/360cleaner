//! 360Cleaner — убирает фотографа и людей с 360° панорамы по двум кадрам с одной точки.
//! Окно: tao + wry (системный WebView), обработка: Rayon. Без нейросетей.
//!
//! Потоки. WebView живёт только в главном потоке, поэтому всё, что идёт в окно, отправляется событием
//! `UserEvent::Js` через `EventLoopProxy` и выполняется там. Диалоги выбора файла тоже открываются в главном потоке
//! (на Mac иначе нельзя). Расчёт превью — один рабочий поток: из очереди берётся только последний запрос, а результат
//! устаревшего расчёта (фото или ползунки успели поменяться) не показывается.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod donations;
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
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use update::{Release, format_version};
use wry::{DragDropEvent, WebViewBuilder};

const IMAGE_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "tif", "tiff"];

#[derive(Debug)]
enum UserEvent {
    /// Скрипт для WebView (выполняется в главном потоке).
    Js(String),
    PickFile(usize),
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

struct Slot {
    path: PathBuf,
    preview: RgbImage,
}

/// Результат расчёта, по которому сохраняется файл.
struct Computed {
    generation: u64,
    alpha: Alpha,
}

#[derive(Default)]
struct UpdateState {
    checking: bool,
    manual: bool,
    downloading: bool,
    available: Option<Release>,
}

struct State {
    slots: [Option<Slot>; 2],
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
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Deserialize)]
struct Request {
    action: String,
    #[serde(default)]
    slot: Option<usize>,
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
    let state: Shared = Arc::new(Mutex::new(State {
        slots: [None, None],
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
    }));

    let (compute_tx, compute_rx) = channel::<()>();
    {
        let (state, ui) = (Arc::clone(&state), ui.clone());
        std::thread::spawn(move || compute_worker(compute_rx, state, ui));
    }

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
                UserEvent::PickFile(slot) => {
                    let lang = lock(&state).lang;
                    let title = i18n::tr(lang, if slot == 0 { i18n::Text::PickFirst } else { i18n::Text::PickSecond });
                    if let Some(file) = rfd::FileDialog::new()
                        .set_title(title)
                        .add_filter(i18n::tr(lang, i18n::Text::Photos), &IMAGE_EXTENSIONS)
                        .pick_file()
                    {
                        load_file(slot, file, &state, &ui, &compute_tx);
                    }
                }
                UserEvent::FilesDropped(paths) => files_dropped(paths, &state, &ui, &compute_tx),
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
            // фото из командной строки: `360Cleaner a.jpg b.jpg`
            let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.is_file()).collect();
            if !paths.is_empty() {
                let _ = proxy.send_event(UserEvent::FilesDropped(paths));
            }
        }
        "pick" => {
            if let Some(slot @ 0..=1) = req.slot {
                let _ = proxy.send_event(UserEvent::PickFile(slot));
            }
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

// ---------------------------------------------------------------- фото

fn files_dropped(paths: Vec<PathBuf>, state: &Shared, ui: &Ui, compute: &Sender<()>) {
    let images: Vec<PathBuf> = paths.into_iter().filter(|p| is_image(p)).collect();
    match images.as_slice() {
        [] => ui.error("errDropTypes", json!({})),
        [one] => {
            let st = lock(state);
            // одно фото — в первый пустой слот, если оба заняты — во второй
            let slot = if st.slots[0].is_none() { 0 } else { 1 };
            drop(st);
            load_file(slot, one.clone(), state, ui, compute);
        }
        [a, b, ..] => {
            load_file(0, a.clone(), state, ui, compute);
            load_file(1, b.clone(), state, ui, compute);
        }
    }
}

fn load_file(slot: usize, path: PathBuf, state: &Shared, ui: &Ui, compute: &Sender<()>) {
    let (state, ui, compute) = (Arc::clone(state), ui.clone(), compute.clone());
    std::thread::spawn(move || {
        let name = file_name(&path);
        ui.send(json!({"type": "slot_loading", "slot": slot, "name": name}));
        ui.status("stLoading", json!({"n": slot + 1, "name": name}), true);
        let full_size = image::image_dimensions(&path).ok();
        match processor::load_image(&path, Some(processor::PREVIEW_DIM)) {
            Ok(preview) => {
                let mut warning = None;
                if let Some((w, h)) = full_size
                    && (w as f32 / h as f32 - 2.0).abs() > 0.05
                {
                    warning = Some(("warnNot360", json!({"name": name, "w": w, "h": h})));
                }
                let image_b64 = jpeg_b64(&preview, 85);
                let mut st = lock(&state);
                st.slots[slot] = Some(Slot { path, preview });
                st.generation += 1;
                st.computed = None;
                if let [Some(a), Some(b)] = &st.slots {
                    let (da, db) = (image::image_dimensions(&a.path).ok(), image::image_dimensions(&b.path).ok());
                    if da != db && warning.is_none() {
                        warning = Some(("warnSizes", json!({})));
                    }
                }
                let both = st.slots.iter().all(Option::is_some);
                drop(st);
                ui.send(json!({
                    "type": "file_loaded", "slot": slot, "name": name, "image_b64": image_b64,
                    "size": full_size.map(|(w, h)| format!("{w}×{h}")),
                }));
                if let Some((key, args)) = warning {
                    ui.send(json!({"type": "warning", "key": key, "args": args}));
                }
                if both {
                    let _ = compute.send(());
                } else {
                    ui.status("stNeedSecond", json!({"name": name}), false);
                }
            }
            Err(e) => {
                ui.send(json!({"type": "slot_failed", "slot": slot}));
                ui.error("errOpen", json!({"name": name, "err": e}));
            }
        }
    });
}

fn compute_worker(rx: Receiver<()>, state: Shared, ui: Ui) {
    while rx.recv().is_ok() {
        // из накопившихся запросов нужен только последний
        while rx.try_recv().is_ok() {}
        let (img1, img2, params, generation) = {
            let st = lock(&state);
            let [Some(a), Some(b)] = &st.slots else { continue };
            if st.computed.as_ref().is_some_and(|c| c.generation == st.generation) {
                continue;
            }
            (a.preview.clone(), b.preview.clone(), st.params, st.generation)
        };
        ui.processing("procPreview");
        let (w, h) = img1.dimensions();
        let img2 = processor::match_size(img2, w, h);
        let started = std::time::Instant::now();
        let preview = processor::compute_preview(&img1, &img2, params);
        let elapsed = started.elapsed().as_millis();

        let mut st = lock(&state);
        if st.generation != generation {
            // фото или параметры поменялись во время расчёта: этот результат никому не нужен
            continue;
        }
        st.computed = Some(Computed { generation, alpha: preview.alpha });
        drop(st);
        ui.send(json!({
            "type": "preview_ready",
            "result_b64": jpeg_b64(&preview.result, 90),
            "mask_b64": jpeg_b64(&preview.mask_vis, 85),
            "angle_a": preview.angles.angle_a.to_degrees(),
            "angle_b": preview.angles.angle_b.to_degrees(),
            "changed": preview.changed,
            "ms": elapsed,
        }));
    }
}

fn save(state: &Shared, ui: &Ui) {
    let st = lock(state);
    if st.exporting {
        return;
    }
    let [Some(a), Some(b)] = &st.slots else {
        ui.error("errNeedBoth", json!({}));
        return;
    };
    let Some(computed) = st.computed.as_ref().filter(|c| c.generation == st.generation) else {
        ui.error("errWaitPreview", json!({}));
        return;
    };
    let (path1, path2, alpha, lang) = (a.path.clone(), b.path.clone(), computed.alpha.clone(), st.lang);
    drop(st);

    let stem = path1.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut dialog = rfd::FileDialog::new()
        .set_title(i18n::tr(lang, i18n::Text::SaveTitle))
        .set_file_name(format!("{stem}_clean.jpg"))
        .add_filter("JPEG", &["jpg", "jpeg"]);
    if let Some(dir) = path1.parent() {
        dialog = dialog.set_directory(dir);
    }
    let Some(mut out) = dialog.save_file() else { return };
    if !out.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")) {
        out.set_extension("jpg");
    }
    if out == path1 || out == path2 {
        ui.error("errOverwrite", json!({}));
        return;
    }
    lock(state).exporting = true;

    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        ui.processing("procSave");
        let result = export(&path1, &path2, &alpha, &out);
        let mut st = lock(&state);
        st.exporting = false;
        match result {
            Ok(()) => {
                st.last_saved = Some(out.clone());
                drop(st);
                ui.send(json!({"type": "save_success", "path": out.to_string_lossy(), "name": file_name(&out)}));
            }
            Err(e) => {
                drop(st);
                ui.error("errSave", json!({"err": e}));
            }
        }
    });
}

fn export(path1: &Path, path2: &Path, alpha: &Alpha, out: &Path) -> Result<(), String> {
    let mut base = processor::load_image(path1, None)?;
    let (w, h) = base.dimensions();
    let other = processor::match_size(processor::load_image(path2, None)?, w, h);
    processor::blend_into(&mut base, &other, alpha);
    drop(other);
    let jpeg = jpeg_bytes(&base, 95)?;
    drop(base);
    let markers = std::fs::read(path1).map(|b| xmp::extract_jpeg_markers(&b)).unwrap_or_default();
    let bytes = xmp::inject_markers(&jpeg, &markers);
    // сначала во временный файл рядом, потом переименование: при сбое старый файл не испортится
    let tmp = out.with_extension("jpg.part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_keeps_size_and_360_metadata() {
        let dir = std::env::temp_dir().join(format!("cleaner360_export_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (800u32, 400u32);
        let frame = |px: u32| {
            RgbImage::from_fn(w, h, |x, y| {
                if x.abs_diff(px) < 20 && y > 150 && y < 300 {
                    image::Rgb([220, 30, 30])
                } else {
                    image::Rgb([(x * 255 / w) as u8, 110, (y * 255 / h) as u8])
                }
            })
        };
        let xmp = b"\xFF\xE1\x00\x2Ahttp://ns.adobe.com/xap/1.0/\x00GPano:equirect";
        let mut xmp = xmp.to_vec();
        let len = xmp.len() - 2;
        xmp[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        let (a, b, out) = (dir.join("a.jpg"), dir.join("b.jpg"), dir.join("out.jpg"));
        std::fs::write(&a, xmp::inject_markers(&jpeg_bytes(&frame(200), 95).unwrap(), &[xmp.clone()])).unwrap();
        std::fs::write(&b, jpeg_bytes(&frame(600), 95).unwrap()).unwrap();

        let small = |p: &Path| processor::load_image(p, Some(400)).unwrap();
        let preview =
            processor::compute_preview(&small(&a), &small(&b), Params { dilate: 6, feather: 3, ..Default::default() });
        export(&a, &b, &preview.alpha, &out).unwrap();

        let saved = std::fs::read(&out).unwrap();
        assert_eq!(xmp::extract_jpeg_markers(&saved), vec![xmp]);
        let img = image::load_from_memory(&saved).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (w, h));
        // одна из двух зон без человека: в его месте нет ярко-красного
        let red = |x: u32| (160..290).filter(|&y| img.get_pixel(x, y)[0] > 200 && img.get_pixel(x, y)[1] < 60).count();
        assert!(red(200) == 0 || red(600) == 0, "both people are still there");
        assert!(!dir.join("out.jpg.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

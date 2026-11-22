//! ИИ-дорисовка — отдельный режим, по умолчанию выключен. Закрашенное кистью место заполняется моделью LaMa
//! (Apache-2.0): она дорисовывает фон, которого нет ни на одном кадре. Остальная программа — без нейросетей.
//!
//! Модель (`lama_fp32.onnx`, вход и выход 512×512) скачивается при первом включении режима в кэш программы и
//! проверяется по SHA-256. Считает ONNX Runtime (крейт `ort`, вшит в программу) на процессоре: CoreML на Mac
//! перестраивал модель при каждой загрузке по 30–150 с и считал медленнее.
//!
//! Панорама в развёртке искажена (особенно у полюсов), поэтому каждое закрашенное место дорисовывается в обычном
//! виде с перспективой (`View`) в его сторону и переносится обратно (`Patch::apply`). Кусок считается один раз и при
//! сохранении в полном размере только переносится — файл совпадает с превью, как маски `Alpha`.

use crate::net;
use image::RgbImage;
use rayon::prelude::*;
use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

/// Сторона входа и выхода модели.
pub const SIDE: u32 = 512;
/// Модель в репозитории Carve/LaMa-ONNX, закреплённая на коммите: файл по ссылке не поменяется.
pub const MODEL_URL: &str =
    "https://huggingface.co/Carve/LaMa-ONNX/resolve/c3c0c9e468934d62e79c329e35d82dd09ff8c444/lama_fp32.onnx";
pub const MODEL_SHA256: &str = "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6";
pub const MODEL_BYTES: u64 = 208_044_816;
/// Какую долю половины вида занимает закрашенное место по большей оси: остальное — фон, по которому модель
/// дорисовывает. Поле зрения вида — от размера места: у широкой перспективы центр сжат, и место на виде вышло бы
/// мелким и грубым. На настоящем фото (человек в 2 м, 55° по высоте): при 0.45 заливка выходит «кубиками», при
/// 0.6–0.75 продолжены решётка и табличка; 0.7 — почти так же резко, а у круглых мест чуть больше фона.
const HOLE: f32 = 0.7;
/// Самое узкое поле зрения: у мелкого места — хотя бы столько фона.
const MIN_FOV: f32 = 24.0 * PI / 180.0;
/// Самое большое место — до 62° от центра по оси (tan 62° ≈ 1.9): дальше перспектива рвётся.
const MAX_EXTENT: f32 = 1.9;
/// Поле зрения, которым меряется место (любое — от него не зависит).
const FRAME_FOV: f32 = PI / 2.0;
/// Насколько расширяется маска в виде (точек вида): край человека и тень должны попасть внутрь.
const GROW: i32 = 6;
/// Ширина мягкого края вставки (точек вида).
const SOFT: i32 = 4;

pub fn model_path() -> PathBuf {
    net::cache_dir().join("models").join("lama_fp32.onnx")
}

pub fn model_ready() -> bool {
    std::fs::metadata(model_path()).is_ok_and(|m| m.len() == MODEL_BYTES)
}

/// Скачивает модель: во временный файл, проверка SHA-256, переименование. `progress` — скачано байт.
pub fn download_model(progress: &dyn Fn(u64)) -> Result<(), String> {
    let path = model_path();
    let dir = path.parent().ok_or("bad model path")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = path.with_extension("onnx.part");
    let _ = std::fs::remove_file(&part);
    net::download_with_progress(MODEL_URL, &part, progress)?;
    let actual = crate::update::sha256_file(&part).map_err(|e| e.to_string())?;
    if actual != MODEL_SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the downloaded model does not match the published SHA-256 ({actual} ≠ {MODEL_SHA256})"));
    }
    std::fs::rename(&part, &path).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- модель

/// Заполняет `mask` (SIDE×SIDE, true — дорисовать) на картинке SIDE×SIDE.
pub trait Model {
    fn fill(&mut self, img: &RgbImage, mask: &[bool]) -> Result<RgbImage, String>;
}

/// LaMa через ONNX Runtime. Загрузка — несколько секунд, поэтому держится, пока программа открыта.
pub struct Lama {
    session: ort::session::Session,
    _env: ort::environment::Environment,
}

impl Lama {
    pub fn load(path: &Path) -> Result<Lama, String> {
        use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
        let err = |e: ort::Error| e.to_string();
        let env = ort::init().with_name("360Cleaner").build().map_err(err)?;
        let session = ort::session::Session::builder(&env)
            .map_err(err)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .commit_from_file(path)
            .map_err(err)?;
        Ok(Lama { session, _env: env })
    }
}

impl Model for Lama {
    fn fill(&mut self, img: &RgbImage, mask: &[bool]) -> Result<RgbImage, String> {
        use ort::value::Tensor;
        let n = (SIDE * SIDE) as usize;
        let mut planes = vec![0f32; 3 * n];
        for (i, p) in img.pixels().enumerate() {
            for c in 0..3 {
                planes[c * n + i] = p[c] as f32 / 255.0;
            }
        }
        let holes: Vec<f32> = mask.iter().map(|&m| m as u8 as f32).collect();
        let side = SIDE as usize;
        let err = |e: ort::Error| e.to_string();
        let image = Tensor::from_array(([1usize, 3, side, side], planes.into_boxed_slice())).map_err(err)?;
        let holes = Tensor::from_array(([1usize, 1, side, side], holes.into_boxed_slice())).map_err(err)?;
        let outputs = self.session.run(ort::inputs!["image" => image, "mask" => holes]).map_err(err)?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(err)?;
        if data.len() != 3 * n {
            return Err(format!("unexpected model output: {} values", data.len()));
        }
        // выход модели — 0…255
        Ok(RgbImage::from_fn(SIDE, SIDE, |x, y| {
            let i = (y * SIDE + x) as usize;
            image::Rgb([0, 1, 2].map(|c| data[c * n + i].round().clamp(0.0, 255.0) as u8))
        }))
    }
}

// ---------------------------------------------------------------- геометрия

type Vec3 = [f32; 3];

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: Vec3) -> Vec3 {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// Направление на центр точки (`x`, `y`) панорамы `w`×`h`. Оси как в `cubemap`: x вправо, y вверх, z вперёд.
fn pano_dir(x: f32, y: f32, w: u32, h: u32) -> Vec3 {
    let lon = (x / w as f32 - 0.5) * TAU;
    let lat = (0.5 - y / h as f32) * PI;
    [lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()]
}

/// Обычный вид с перспективой SIDE×SIDE в сторону `fwd`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    right: Vec3,
    up: Vec3,
    fwd: Vec3,
    /// Фокусное расстояние в точках вида.
    f: f32,
}

impl View {
    pub fn toward(center: Vec3, fov: f32) -> View {
        let fwd = normalize(center);
        // у полюса «вверх» берётся вдоль z, иначе вид крутился бы как попало
        let world_up = if fwd[1].abs() > 0.99 { [0.0, 0.0, -fwd[1].signum()] } else { [0.0, 1.0, 0.0] };
        let right = normalize(cross(world_up, fwd));
        let up = cross(fwd, right);
        View { right, up, fwd, f: SIDE as f32 / 2.0 / (fov / 2.0).tan() }
    }

    /// Направление через точку (`x`, `y`) вида (центр точки — +0.5).
    fn ray(&self, x: f32, y: f32) -> Vec3 {
        let (sx, sy) = (x - SIDE as f32 / 2.0, SIDE as f32 / 2.0 - y);
        normalize([0, 1, 2].map(|i| self.fwd[i] * self.f + self.right[i] * sx + self.up[i] * sy))
    }

    /// Где направление `d` на виде; None — позади или за краем.
    fn project(&self, d: Vec3) -> Option<(f32, f32)> {
        let z = dot(d, self.fwd);
        if z <= 1e-4 {
            return None;
        }
        let x = SIDE as f32 / 2.0 + self.f * dot(d, self.right) / z;
        let y = SIDE as f32 / 2.0 - self.f * dot(d, self.up) / z;
        let s = SIDE as f32;
        (x >= 0.0 && y >= 0.0 && x < s && y < s).then_some((x, y))
    }

    /// Косинус угла от центра до угла вида: дальше этого направления на виде нет.
    fn min_cos(&self) -> f32 {
        let half = SIDE as f32 / 2.0 * std::f32::consts::SQRT_2;
        (self.f / half.hypot(self.f)) * 0.999
    }
}

// ---------------------------------------------------------------- маска

/// Мазок кисти: центр (`u`, `v` от 0 до 1 по панораме), радиус в долях ширины, стереть или закрасить.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
pub struct Dab {
    pub u: f32,
    pub v: f32,
    pub r: f32,
    #[serde(default)]
    pub erase: bool,
}

/// Маска `w`×`h` из мазков по порядку; по горизонтали панорама замкнута.
pub fn rasterize(dabs: &[Dab], w: u32, h: u32) -> Vec<bool> {
    let mut mask = vec![false; (w * h) as usize];
    let (wi, hi) = (w as i64, h as i64);
    for d in dabs {
        let (cx, cy, r) = (d.u * w as f32, d.v * h as f32, (d.r * w as f32).max(0.5));
        let ri = r.ceil() as i64;
        let (x0, y0) = (cx.floor() as i64, cy.floor() as i64);
        for y in (y0 - ri).max(0)..=(y0 + ri).min(hi - 1) {
            for x in x0 - ri..=x0 + ri {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if dx * dx + dy * dy <= r * r {
                    mask[(y * wi + x.rem_euclid(wi)) as usize] = !d.erase;
                }
            }
        }
    }
    mask
}

/// Связные куски маски (8 соседей, по горизонтали замкнуто): номера точек каждого куска.
fn regions(mask: &[bool], w: u32, h: u32) -> Vec<Vec<usize>> {
    let (w, h) = (w as usize, h as usize);
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let (mut stack, mut region) = (vec![start], Vec::new());
        while let Some(i) = stack.pop() {
            region.push(i);
            let (x, y) = (i % w, i / w);
            for dy in -1i64..=1 {
                let ny = y as i64 + dy;
                if ny < 0 || ny >= h as i64 {
                    continue;
                }
                for dx in -1i64..=1 {
                    let nx = (x as i64 + dx).rem_euclid(w as i64) as usize;
                    let j = ny as usize * w + nx;
                    if mask[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(region);
    }
    out
}

/// Сторона и поле зрения вида для куска: центр — среднее направление, поле — `CONTEXT` × его угловой размер.
fn view_for(region: &[usize], w: u32, h: u32) -> Result<View, String> {
    let dirs: Vec<Vec3> =
        region.iter().map(|&i| pano_dir((i % w as usize) as f32 + 0.5, (i / w as usize) as f32 + 0.5, w, h)).collect();
    let sum = dirs.iter().fold([0f32; 3], |s, d| [s[0] + d[0], s[1] + d[1], s[2] + d[2]]);
    let view = View::toward(normalize(sum), FRAME_FOV);
    // размер места на плоскости вида (в долях фокусного расстояния) по большей оси
    let mut extent = 0f32;
    for d in &dirs {
        let z = dot(*d, view.fwd);
        if z < MAX_EXTENT.atan().cos() {
            return Err("too_big".into());
        }
        extent = extent.max((dot(*d, view.right) / z).abs()).max((dot(*d, view.up) / z).abs());
    }
    // плюс точка превью — чтобы и один мазок получил разумный вид
    let extent = extent + (PI / h as f32).tan();
    if extent > MAX_EXTENT {
        return Err("too_big".into());
    }
    // место занимает долю `HOLE` половины вида; остальное — фон для модели
    let half = SIDE as f32 / 2.0;
    let f = (HOLE * half / extent).min(half / (MIN_FOV / 2.0).tan());
    Ok(View { f, ..view })
}

// ---------------------------------------------------------------- вставка

/// Дорисованный кусок: вид, картинка модели и мягкая маска вставки (SIDE×SIDE).
#[derive(Clone)]
pub struct Patch {
    view: View,
    image: RgbImage,
    alpha: Vec<f32>,
}

fn bilinear(data: &[f32], x: f32, y: f32) -> f32 {
    let s = SIDE as usize;
    let (fx, fy) = ((x - 0.5).clamp(0.0, (s - 1) as f32), (y - 0.5).clamp(0.0, (s - 1) as f32));
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(s - 1), (y0 + 1).min(s - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let top = data[y0 * s + x0] * (1.0 - tx) + data[y0 * s + x1] * tx;
    let bottom = data[y1 * s + x0] * (1.0 - tx) + data[y1 * s + x1] * tx;
    top * (1.0 - ty) + bottom * ty
}

fn bilinear_rgb(img: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let s = SIDE as usize;
    let (fx, fy) = ((x - 0.5).clamp(0.0, (s - 1) as f32), (y - 0.5).clamp(0.0, (s - 1) as f32));
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(s - 1), (y0 + 1).min(s - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let raw = img.as_raw();
    let at = |x: usize, y: usize, c: usize| raw[(y * s + x) * 3 + c] as f32;
    [0, 1, 2].map(|c| {
        let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
        let bottom = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
        top * (1.0 - ty) + bottom * ty
    })
}

/// Квадрат вокруг каждой точки (расширение маски).
fn grow(mask: &[bool], r: i32) -> Vec<bool> {
    let s = SIDE as i32;
    let mut out = vec![false; mask.len()];
    for y in 0..s {
        for x in 0..s {
            if !mask[(y * s + x) as usize] {
                continue;
            }
            for ny in (y - r).max(0)..=(y + r).min(s - 1) {
                for nx in (x - r).max(0)..=(x + r).min(s - 1) {
                    if (nx - x).pow(2) + (ny - y).pow(2) <= r * r {
                        out[(ny * s + nx) as usize] = true;
                    }
                }
            }
        }
    }
    out
}

/// Размытие прямоугольником по горизонтали и вертикали.
fn soften(mask: &[bool], r: i32) -> Vec<f32> {
    let s = SIDE as i32;
    let src: Vec<f32> = mask.iter().map(|&m| m as u8 as f32).collect();
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; src.len()];
        for a in 0..s {
            for b in 0..s {
                let (mut sum, mut n) = (0f32, 0f32);
                for k in (b - r).max(0)..=(b + r).min(s - 1) {
                    let (x, y) = if horizontal { (k, a) } else { (a, k) };
                    sum += src[(y * s + x) as usize];
                    n += 1.0;
                }
                let (x, y) = if horizontal { (b, a) } else { (a, b) };
                out[(y * s + x) as usize] = sum / n;
            }
        }
        out
    };
    pass(&pass(&src, true), false)
}

impl Patch {
    /// Вид `view` из панорамы `pano` (любого размера) с маской `mask` (в размере `mw`×`mh`), заполненный моделью.
    fn make(
        view: View,
        pano: &RgbImage,
        mask: &[bool],
        mw: u32,
        mh: u32,
        model: &mut dyn Model,
    ) -> Result<Patch, String> {
        let s = SIDE as usize;
        let mut img = RgbImage::new(SIDE, SIDE);
        let mut holes = vec![false; s * s];
        img.par_chunks_mut(s * 3).zip(holes.par_chunks_mut(s)).enumerate().for_each(|(y, (row, hrow))| {
            for x in 0..s {
                let d = view.ray(x as f32 + 0.5, y as f32 + 0.5);
                row[x * 3..x * 3 + 3].copy_from_slice(&crate::cubemap::sample(pano, d));
                let (lon, lat) = (d[0].atan2(d[2]), d[1].atan2(d[0].hypot(d[2])));
                let mx = ((lon / TAU + 0.5) * mw as f32).floor() as i64;
                let my = (((0.5 - lat / PI) * mh as f32).floor() as i64).clamp(0, mh as i64 - 1);
                hrow[x] = mask[my as usize * mw as usize + mx.rem_euclid(mw as i64) as usize];
            }
        });
        let holes = grow(&holes, GROW);
        let filled = model.fill(&img, &holes)?;
        // внутри маски — ровно 1: маска расширяется на диагональ окна размытия, мягкий край уходит на фон
        let alpha = soften(&grow(&holes, 2 * SOFT), SOFT);
        Ok(Patch { view, image: filled, alpha })
    }

    /// Переносит кусок на панораму любого размера.
    pub fn apply(&self, pano: &mut RgbImage) {
        let (w, h) = pano.dimensions();
        let min_cos = self.view.min_cos();
        let lons: Vec<(f32, f32)> =
            (0..w).map(|x| ((x as f32 + 0.5) / w as f32 - 0.5) * TAU).map(|l| (l.sin(), l.cos())).collect();
        pano.par_chunks_mut(w as usize * 3).enumerate().for_each(|(y, row)| {
            let lat = (0.5 - (y as f32 + 0.5) / h as f32) * PI;
            let (sl, cl) = lat.sin_cos();
            for (x, &(so, co)) in lons.iter().enumerate() {
                let d = [cl * so, sl, cl * co];
                if dot(d, self.view.fwd) < min_cos {
                    continue;
                }
                let Some((vx, vy)) = self.view.project(d) else { continue };
                let a = bilinear(&self.alpha, vx, vy);
                if a <= 0.0 {
                    continue;
                }
                let c = bilinear_rgb(&self.image, vx, vy);
                let px = &mut row[x * 3..x * 3 + 3];
                for k in 0..3 {
                    px[k] = (px[k] as f32 * (1.0 - a) + c[k] * a).round().clamp(0.0, 255.0) as u8;
                }
            }
        });
    }
}

/// Дорисовывает закрашенное `mask` (в размере `mw`×`mh`): каждый связный кусок — своим видом, по очереди, так что
/// следующий вид уже видит дорисованное. `pano` — результат в размере, по которому дорисовывать (от него зависит
/// резкость фона на виде), на нём куски и остаются. `on_region(i, n)` — перед каждым куском.
pub fn fill(
    pano: &mut RgbImage,
    mask: &[bool],
    mw: u32,
    mh: u32,
    model: &mut dyn Model,
    on_region: &dyn Fn(usize, usize),
) -> Result<Vec<Patch>, String> {
    let parts = regions(mask, mw, mh);
    let mut patches = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        on_region(i, parts.len());
        let view = view_for(part, mw, mh)?;
        let patch = Patch::make(view, pano, mask, mw, mh, model)?;
        patch.apply(pano);
        patches.push(patch);
    }
    Ok(patches)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// «Модель» для тестов: дыру заливает одним цветом.
    struct Flat([u8; 3]);

    impl Model for Flat {
        fn fill(&mut self, img: &RgbImage, mask: &[bool]) -> Result<RgbImage, String> {
            let mut out = img.clone();
            for (p, &m) in out.pixels_mut().zip(mask) {
                if m {
                    p.0 = self.0;
                }
            }
            Ok(out)
        }
    }

    fn pano(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, 100, (y * 255 / h) as u8]))
    }

    #[test]
    fn view_projects_its_own_rays() {
        for center in [[0.0, 0.0, 1.0], [1.0, 0.3, -0.2], [0.0, -1.0, 0.0], [0.1, 0.999, 0.0]] {
            let v = View::toward(center, 60f32.to_radians());
            for (x, y) in [(0.5, 0.5), (256.0, 256.0), (100.3, 400.7), (511.5, 3.2)] {
                let (px, py) = v.project(v.ray(x, y)).expect("on the view");
                assert!((px - x).abs() < 0.05 && (py - y).abs() < 0.05, "{center:?} ({x},{y}) → ({px},{py})");
            }
            assert!(v.project(normalize(center.map(|c| -c))).is_none());
        }
    }

    #[test]
    fn dabs_wrap_around_and_erase() {
        let (w, h) = (100u32, 50u32);
        // мазок через шов панорамы
        let m = rasterize(&[Dab { u: 0.995, v: 0.5, r: 0.03, erase: false }], w, h);
        assert!(m[(25 * w) as usize] && m[(25 * w + 99) as usize] && m[(25 * w + 1) as usize]);
        assert!(!m[(25 * w + 50) as usize]);
        let m = rasterize(
            &[Dab { u: 0.5, v: 0.5, r: 0.1, erase: false }, Dab { u: 0.5, v: 0.5, r: 0.02, erase: true }],
            w,
            h,
        );
        assert!(!m[(25 * w + 50) as usize] && m[(25 * w + 45) as usize]);
        // кусок через шов — один
        let m = rasterize(
            &[Dab { u: 0.0, v: 0.5, r: 0.05, erase: false }, Dab { u: 0.5, v: 0.5, r: 0.05, erase: false }],
            w,
            h,
        );
        assert_eq!(regions(&m, w, h).len(), 2);
    }

    #[test]
    fn fill_changes_only_the_painted_place() {
        let (w, h) = (800u32, 400u32);
        let mut img = pano(w, h);
        let original = img.clone();
        // два места: у экватора и у нижнего полюса (там развёртка искажена сильнее всего)
        let dabs = [Dab { u: 0.3, v: 0.5, r: 0.02, erase: false }, Dab { u: 0.8, v: 0.93, r: 0.02, erase: false }];
        let mask = rasterize(&dabs, w, h);
        let patches = fill(&mut img, &mask, w, h, &mut Flat([255, 0, 255]), &|_, _| {}).unwrap();
        assert_eq!(patches.len(), 2);
        for (i, (p, q)) in img.pixels().zip(original.pixels()).enumerate() {
            let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
            let near = dabs.iter().any(|d| {
                let dx = ((x / w as f32 - d.u + 0.5).rem_euclid(1.0) - 0.5) * w as f32;
                let dy = y - d.v * h as f32;
                // у полюса точка развёртки уже по широте: запас по горизонтали больше
                (dx * (y / h as f32 * PI).sin()).hypot(dy) < d.r * w as f32 + 14.0
            });
            if mask[i] {
                assert_eq!(p.0, [255, 0, 255], "painted point ({x},{y}) not filled");
            } else if !near {
                assert_eq!(p.0, q.0, "point ({x},{y}) far from the mask changed");
            }
        }
        // тот же кусок на панораме вдвое больше ложится на то же место
        let mut big = pano(w * 2, h * 2);
        for p in &patches {
            p.apply(&mut big);
        }
        let (cx, cy) = ((0.3 * w as f32 * 2.0) as u32, h);
        assert_eq!(big.get_pixel(cx, cy).0, [255, 0, 255]);
        assert_eq!(big.get_pixel(cx + 120, cy).0, pano(w * 2, h * 2).get_pixel(cx + 120, cy).0);
    }

    #[test]
    fn too_big_place_is_refused() {
        let (w, h) = (200u32, 100u32);
        let mask = rasterize(&[Dab { u: 0.5, v: 0.5, r: 0.3, erase: false }], w, h);
        let err = fill(&mut pano(w, h), &mask, w, h, &mut Flat([0; 3]), &|_, _| {}).err();
        assert_eq!(err.as_deref(), Some("too_big"));
    }

    /// Дорисовка на настоящем фото: `CLEANER360_LAMA=lama_fp32.onnx CLEANER360_SAMPLE=photo.jpg
    /// CLEANER360_DABS="u,v,r;u,v,r" CLEANER360_OUT=out.jpg cargo test --release -- --ignored --nocapture lama_photo`
    #[test]
    #[ignore]
    fn lama_photo() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let dabs: Vec<Dab> = var("CLEANER360_DABS")
            .split(';')
            .map(|d| {
                let v: Vec<f32> = d.split(',').map(|x| x.trim().parse().unwrap()).collect();
                Dab { u: v[0], v: v[1], r: v[2], erase: false }
            })
            .collect();
        let photo = std::path::PathBuf::from(var("CLEANER360_SAMPLE"));
        let preview = crate::processor::load_image(&photo, Some(crate::processor::PREVIEW_DIM)).unwrap();
        let (pw, ph) = preview.dimensions();
        let mask = rasterize(&dabs, pw, ph);
        let (mut big, ..) = crate::export::compose(std::slice::from_ref(&photo), &[], 8192).unwrap();
        let mut lama = Lama::load(Path::new(&var("CLEANER360_LAMA"))).unwrap();
        let t = std::time::Instant::now();
        let patches = fill(&mut big, &mask, pw, ph, &mut lama, &|i, n| println!("region {}/{n}", i + 1)).unwrap();
        println!("fill {:?}, {} regions", t.elapsed(), patches.len());
        big.save(var("CLEANER360_OUT")).unwrap();
    }

    /// `CLEANER360_LAMA=lama_fp32.onnx cargo test --release -- --ignored --nocapture lama_runs`
    #[test]
    #[ignore]
    fn lama_runs() {
        let path = std::env::var("CLEANER360_LAMA").expect("CLEANER360_LAMA");
        let t = std::time::Instant::now();
        let mut lama = Lama::load(Path::new(&path)).unwrap();
        println!("load {:?}", t.elapsed());
        let img = RgbImage::from_fn(SIDE, SIDE, |x, _| image::Rgb([(x / 2) as u8, 120, 200]));
        let mask: Vec<bool> =
            (0..SIDE * SIDE).map(|i| (i % SIDE).abs_diff(256) < 40 && (i / SIDE).abs_diff(256) < 40).collect();
        let t = std::time::Instant::now();
        let out = lama.fill(&img, &mask).unwrap();
        println!("fill {:?}", t.elapsed());
        // гладкий градиент модель продолжает гладко
        let p = out.get_pixel(256, 256).0;
        assert!(p[0].abs_diff(128) < 30 && p[1].abs_diff(120) < 30 && p[2].abs_diff(200) < 30, "{p:?}");
    }
}

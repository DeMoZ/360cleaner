//! Обработка: равнопромежуточные (equirectangular) 360° панорамы с одной точки → одна без людей.
//! Классическое компьютерное зрение без нейросетей, параллельно через Rayon.
//!
//! Два кадра (`compute_preview`) — фотограф на каждом в своём месте; три и больше (`compute_stack_preview`) —
//! прохожие, фон берётся как медиана кадров.
//!
//! Шаги для двух кадров: разница в цветовом пространстве Lab → порог → убрать одиночные точки шума → расширить маску кругом
//! (по горизонтали панорама замкнута) → два угловых кластера на окружности (где стоял человек на каждом кадре) →
//! мягкая маска «брать пиксель из второго кадра» → смешивание.
//!
//! Маска считается один раз по превью и для экспорта растягивается до полного размера, поэтому сохранённый файл
//! совпадает с тем, что видно в окне.

use image::RgbImage;
use rayon::prelude::*;
use std::f32::consts::PI;
use std::path::Path;

/// Ширина, до которой уменьшаются кадры для превью и расчёта маски.
pub const PREVIEW_DIM: u32 = 1400;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Порог разницы ΔE (Lab).
    pub threshold: f32,
    /// Радиус расширения маски, пикселей превью.
    pub dilate: i32,
    /// Радиус смягчения шва, пикселей превью.
    pub feather: i32,
    /// Поменять, из какого кадра берётся каждая зона.
    pub swap: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { threshold: 24.0, dilate: 25, feather: 20, swap: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

#[inline(always)]
fn rgb_to_lab(r: u8, g: u8, b: u8) -> Lab {
    let to_linear = |c: u8| -> f32 {
        let v = (c as f32) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (lr, lg, lb) = (to_linear(r), to_linear(g), to_linear(b));
    // наблюдатель 2°, источник D65
    let x = (lr * 0.412_456_4 + lg * 0.357_576_1 + lb * 0.180_437_5) / 0.95047;
    let y = lr * 0.212_672_9 + lg * 0.715_152_2 + lb * 0.072_175;
    let z = (lr * 0.019_333_9 + lg * 0.119_192 + lb * 0.950_304_1) / 1.08883;
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    Lab { l: 116.0 * fy - 16.0, a: 500.0 * (fx - fy), b: 200.0 * (fy - fz) }
}

/// Расстояние между углами на окружности, радианы, [0, π].
#[inline(always)]
pub fn circular_distance(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs() % (2.0 * PI);
    if diff > PI { 2.0 * PI - diff } else { diff }
}

/// Открывает файл; с `max_dim` уменьшает так, чтобы большая сторона была не больше `max_dim`.
pub fn load_image(path: &Path, max_dim: Option<u32>) -> Result<RgbImage, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    Ok(match max_dim {
        Some(dim) => downscale(img, dim),
        None => img,
    })
}

pub fn downscale(img: RgbImage, dim: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    if w <= dim && h <= dim {
        return img;
    }
    let scale = dim as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

/// Второй кадр того же размера, что первый.
pub fn match_size(img: RgbImage, w: u32, h: u32) -> RgbImage {
    if img.dimensions() == (w, h) {
        img
    } else {
        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
    }
}

/// Разница ΔE76 между кадрами по пикселям.
pub fn compute_difference_map(img1: &RgbImage, img2: &RgbImage) -> Vec<f32> {
    let w = img1.width() as usize;
    let (a, b) = (img1.as_raw(), img2.as_raw());
    let mut diff = vec![0.0f32; w * img1.height() as usize];
    diff.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let base = y * w * 3;
        for (x, out) in row.iter_mut().enumerate() {
            let i = base + x * 3;
            let p = rgb_to_lab(a[i], a[i + 1], a[i + 2]);
            let q = rgb_to_lab(b[i], b[i + 1], b[i + 2]);
            let (dl, da, db) = (p.l - q.l, p.a - q.a, p.b - q.b);
            *out = (dl * dl + da * da + db * db).sqrt();
        }
    });
    diff
}

/// Для каждого пикселя строки — расстояние по горизонтали до ближайшего `true` с учётом замкнутости панорамы,
/// не больше `cap`.
fn row_distances(row: &[bool], cap: u32, out: &mut [u32]) {
    let w = row.len();
    if !row.iter().any(|&b| b) {
        out.fill(cap);
        return;
    }
    // два прохода по удвоенной строке: слева направо и справа налево
    let mut d = cap;
    for i in 0..2 * w {
        let x = i % w;
        d = if row[x] { 0 } else { (d + 1).min(cap) };
        if i >= w {
            out[x] = d;
        }
    }
    d = cap;
    for i in (0..2 * w).rev() {
        let x = i % w;
        d = if row[x] { 0 } else { (d + 1).min(cap) };
        if i < w {
            out[x] = out[x].min(d);
        }
    }
}

/// Расширение маски кругом радиуса `radius`; по горизонтали панорама замкнута, по вертикали — нет.
/// Работает за O(w·h·r): расстояния по строкам считаются один раз.
pub fn dilate_mask_360(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<bool> {
    if radius <= 0 {
        return mask.to_vec();
    }
    let (w, h, r) = (w as usize, h as usize, radius as usize);
    let cap = (r + 1) as u32;
    let mut dist = vec![0u32; w * h];
    dist.par_chunks_mut(w).zip(mask.par_chunks(w)).for_each(|(out, row)| row_distances(row, cap, out));
    // половина ширины круга на расстоянии dy от центра
    let reach: Vec<u32> = (0..=r).map(|dy| (((r * r - dy * dy) as f64).sqrt().floor()) as u32).collect();
    let mut result = vec![false; w * h];
    result.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let y0 = y.saturating_sub(r);
        let y1 = (y + r).min(h - 1);
        for (x, o) in out.iter_mut().enumerate() {
            *o = (y0..=y1).any(|ny| dist[ny * w + x] <= reach[ny.abs_diff(y)]);
        }
    });
    result
}

/// Сужение маски (обратное расширению): убирает точки шума меньше круга радиуса `radius`.
pub fn erode_mask_360(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<bool> {
    let inverted: Vec<bool> = mask.iter().map(|&b| !b).collect();
    dilate_mask_360(&inverted, w, h, radius).into_iter().map(|b| !b).collect()
}

/// Центры двух зон различий на окружности: где на первом кадре стоял человек и где на втором.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterAngles {
    /// [0, 2π), всегда `angle_a < angle_b`.
    pub angle_a: f32,
    pub angle_b: f32,
}

const BINS: usize = 360;

/// k-средних на окружности с детерминированным началом: самый плотный столбец гистограммы углов и самый плотный
/// из тех, что дальше 30° от него.
pub fn find_cluster_angles(mask: &[bool], w: u32, h: u32) -> ClusterAngles {
    let (w, h) = (w as usize, h as usize);
    let mut columns = vec![0u32; w];
    for y in 0..h {
        for (x, c) in columns.iter_mut().enumerate() {
            *c += mask[y * w + x] as u32;
        }
    }
    let total: u32 = columns.iter().sum();
    if total < 20 {
        return ClusterAngles { angle_a: 0.0, angle_b: PI };
    }
    let theta = |x: usize| 2.0 * PI * (x as f32 + 0.5) / w as f32;

    let mut hist = [0f32; BINS];
    for (x, &c) in columns.iter().enumerate() {
        hist[(x * BINS / w).min(BINS - 1)] += c as f32;
    }
    // сглаживание ±5°
    let smooth: Vec<f32> = (0..BINS).map(|i| (0..11).map(|k| hist[(i + BINS + k - 5) % BINS]).sum()).collect();
    let bin_angle = |i: usize| 2.0 * PI * (i as f32 + 0.5) / BINS as f32;
    let first = (0..BINS).max_by(|&a, &b| smooth[a].total_cmp(&smooth[b])).unwrap_or(0);
    let a0 = bin_angle(first);
    let second = (0..BINS)
        .filter(|&i| circular_distance(bin_angle(i), a0) > 30f32.to_radians() && smooth[i] > 0.0)
        .max_by(|&a, &b| smooth[a].total_cmp(&smooth[b]));
    let a1 = second.map(bin_angle).unwrap_or(a0 + PI);

    let mut c0 = (a0.cos(), a0.sin());
    let mut c1 = (a1.cos(), a1.sin());
    for _ in 0..20 {
        let (mut s0, mut s1) = ((0f32, 0f32), (0f32, 0f32));
        for (x, &n) in columns.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let t = theta(x);
            let (c, s) = (t.cos(), t.sin());
            let n = n as f32;
            if c * c0.0 + s * c0.1 >= c * c1.0 + s * c1.1 {
                s0 = (s0.0 + c * n, s0.1 + s * n);
            } else {
                s1 = (s1.0 + c * n, s1.1 + s * n);
            }
        }
        let norm = |v: (f32, f32)| (v.0 * v.0 + v.1 * v.1).sqrt();
        if norm(s0) > 1e-5 {
            c0 = (s0.0 / norm(s0), s0.1 / norm(s0));
        }
        if norm(s1) > 1e-5 {
            c1 = (s1.0 / norm(s1), s1.1 / norm(s1));
        }
    }
    let mut ang0 = c0.1.atan2(c0.0).rem_euclid(2.0 * PI);
    let mut ang1 = c1.1.atan2(c1.0).rem_euclid(2.0 * PI);
    if ang0 > ang1 {
        std::mem::swap(&mut ang0, &mut ang1);
    }
    ClusterAngles { angle_a: ang0, angle_b: ang1 }
}

/// Относится ли столбец `x` к зоне A.
#[inline(always)]
fn in_zone_a(x: usize, w: usize, angles: ClusterAngles) -> bool {
    let t = 2.0 * PI * (x as f32 + 0.5) / w as f32;
    circular_distance(t, angles.angle_a) <= circular_distance(t, angles.angle_b)
}

/// Маска «взять пиксель из второго кадра» до смягчения: без swap — зона A, со swap — зона B.
pub fn target_mask(mask: &[bool], w: u32, angles: ClusterAngles, swap: bool) -> Vec<bool> {
    let w = w as usize;
    let zone: Vec<bool> = (0..w).map(|x| in_zone_a(x, w, angles) != swap).collect();
    mask.iter().enumerate().map(|(i, &m)| m && zone[i % w]).collect()
}

/// Смягчение маски: размытие прямоугольником по горизонтали (замкнуто) и по вертикали.
pub fn feather_alpha(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<f32> {
    let alpha: Vec<f32> = mask.iter().map(|&b| b as u8 as f32).collect();
    if radius <= 0 {
        return alpha;
    }
    let (w, h) = (w as usize, h as usize);
    let r = (radius as usize).min(w / 2).max(1);
    let diameter = (2 * r + 1) as f32;

    let mut temp = vec![0f32; w * h];
    temp.par_chunks_mut(w).zip(alpha.par_chunks(w)).for_each(|(out, row)| {
        let mut sum: f32 = (0..=2 * r).map(|k| row[(k + w - r) % w]).sum();
        out[0] = sum / diameter;
        for x in 1..w {
            sum += row[(x + r) % w] - row[(x + w - r - 1) % w];
            out[x] = (sum / diameter).clamp(0.0, 1.0);
        }
    });
    let mut result = vec![0f32; w * h];
    result.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let y0 = y.saturating_sub(r);
        let y1 = (y + r).min(h - 1);
        let n = (y1 - y0 + 1) as f32;
        for (x, o) in out.iter_mut().enumerate() {
            let sum: f32 = (y0..=y1).map(|ny| temp[ny * w + x]).sum();
            *o = (sum / n).clamp(0.0, 1.0);
        }
    });
    result
}

/// Мягкая маска в размере превью.
#[derive(Clone, Debug)]
pub struct Alpha {
    pub w: u32,
    pub h: u32,
    pub data: Vec<f32>,
}

impl Alpha {
    /// Значение в точке (u, v) ∈ [0,1)² с билинейной интерполяцией; по горизонтали замкнуто.
    #[inline(always)]
    fn sample(&self, u: f32, v: f32) -> f32 {
        let (w, h) = (self.w as usize, self.h as usize);
        let fx = u * w as f32 - 0.5;
        let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let x0f = fx.floor();
        let tx = fx - x0f;
        let x0 = (x0f as i64).rem_euclid(w as i64) as usize;
        let x1 = (x0 + 1) % w;
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(h - 1);
        let ty = fy - y0 as f32;
        let d = &self.data;
        let top = d[y0 * w + x0] * (1.0 - tx) + d[y0 * w + x1] * tx;
        let bottom = d[y1 * w + x0] * (1.0 - tx) + d[y1 * w + x1] * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

pub struct Preview {
    pub result: RgbImage,
    pub mask_vis: RgbImage,
    pub alpha: Alpha,
    pub angles: ClusterAngles,
    /// Доля пикселей, где кадры различаются.
    pub changed: f32,
}

/// Полный расчёт по превью. Кадры одного размера.
pub fn compute_preview(img1: &RgbImage, img2: &RgbImage, p: Params) -> Preview {
    let (w, h) = img1.dimensions();
    let diff = compute_difference_map(img1, img2);
    // одиночные точки (шум JPEG, листья) не должны раздуваться расширением
    let mask = changed_mask(&diff, w, h, p);
    let angles = find_cluster_angles(&mask, w, h);
    let target = target_mask(&mask, w, angles, p.swap);
    let alpha = Alpha { w, h, data: feather_alpha(&target, w, h, p.feather) };
    let mut result = img1.clone();
    blend_into(&mut result, img2, &alpha);
    let changed = mask.iter().filter(|&&m| m).count() as f32 / mask.len().max(1) as f32;
    Preview { result, mask_vis: mask_visualization(&mask, w, h, angles), alpha, angles, changed }
}

/// Смешивает `img2` в `base` по мягкой маске любого размера (растягивается до размера кадра).
pub fn blend_into(base: &mut RgbImage, img2: &RgbImage, alpha: &Alpha) {
    let (w, h) = base.dimensions();
    let same = alpha.w == w && alpha.h == h;
    let row_len = w as usize * 3;
    let src = img2.as_raw();
    base.par_chunks_mut(row_len).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w as usize {
            let a = if same { alpha.data[y * w as usize + x] } else { alpha.sample((x as f32 + 0.5) / w as f32, v) };
            if a <= 0.0 {
                continue;
            }
            let i = x * 3;
            let j = y * row_len + i;
            for c in 0..3 {
                let p1 = row[i + c] as f32;
                let p2 = src[j + c] as f32;
                row[i + c] = ((1.0 - a) * p1 + a * p2).round() as u8;
            }
        }
    });
}

// ---------------------------------------------------------------- три кадра и больше

/// Цвета кадров на карте различий (и у фото в списке окна): кадр `i` — `SOURCE_COLORS[i % len]`.
pub const SOURCE_COLORS: [[u8; 3]; 8] = [
    [56, 189, 248],
    [239, 68, 68],
    [16, 185, 129],
    [245, 158, 11],
    [139, 92, 246],
    [236, 72, 153],
    [234, 179, 8],
    [20, 184, 166],
];

/// Медиана по кадрам для каждого канала каждого пикселя: то, что видно на большинстве кадров (фон без прохожих).
pub fn median_image(frames: &[RgbImage]) -> RgbImage {
    let (w, h) = frames[0].dimensions();
    let n = frames.len();
    let row_len = w as usize * 3;
    let mut out = RgbImage::new(w, h);
    out.par_chunks_mut(row_len).enumerate().for_each(|(y, row)| {
        let mut values = vec![0u8; n];
        let base = y * row_len;
        for (i, o) in row.iter_mut().enumerate() {
            for (v, f) in values.iter_mut().zip(frames) {
                *v = f.as_raw()[base + i];
            }
            values.sort_unstable();
            *o = if n % 2 == 1 {
                values[n / 2]
            } else {
                (values[n / 2 - 1] as u16 + values[n / 2] as u16).div_ceil(2) as u8
            };
        }
    });
    out
}

/// Маска различий одного кадра с фоном: порог, удаление точек шума, расширение.
fn changed_mask(diff: &[f32], w: u32, h: u32, p: Params) -> Vec<bool> {
    let raw: Vec<bool> = diff.iter().map(|&d| d > p.threshold).collect();
    let opened = dilate_mask_360(&erode_mask_360(&raw, w, h, 1), w, h, 1);
    dilate_mask_360(&opened, w, h, p.dilate)
}

/// Маски «взять кадр i» для `blend_sequence`: из долей кадров `weights` (в каждой точке сумма 1).
/// После смешивания кадров 1..=i по очереди доли совпадают с `weights`.
fn sequential_alphas(weights: &[Vec<f32>], w: u32, h: u32) -> Vec<Alpha> {
    let mut sum = weights[0].clone();
    weights[1..]
        .iter()
        .map(|wi| {
            let data = sum
                .iter_mut()
                .zip(wi)
                .map(|(s, &v)| {
                    *s += v;
                    if *s > 1e-6 { (v / *s).clamp(0.0, 1.0) } else { 0.0 }
                })
                .collect();
            Alpha { w, h, data }
        })
        .collect()
}

/// Смешивает в `base` (кадр 0) кадры 1.. по их маскам из `compute_stack_preview` / `compute_preview`.
pub fn blend_sequence(base: &mut RgbImage, others: &[RgbImage], alphas: &[Alpha]) {
    for (img, alpha) in others.iter().zip(alphas) {
        blend_into(base, img, alpha);
    }
}

pub struct StackPreview {
    pub result: RgbImage,
    pub mask_vis: RgbImage,
    /// Маски кадров 1.. для `blend_sequence`.
    pub alphas: Vec<Alpha>,
    /// Доля пикселей, где хоть один кадр отличается от фона.
    pub changed: f32,
}

/// Расчёт для трёх кадров и больше (все одного размера). Фон — медиана кадров; на каждом кадре отмечается, где он
/// отличается от фона (там прохожий). В каждой точке берётся первый по списку кадр, где там чисто, а если чисто
/// нигде — самый близкий к фону. Границы между кадрами смягчаются.
pub fn compute_stack_preview(frames: &[RgbImage], p: Params) -> StackPreview {
    let (w, h) = frames[0].dimensions();
    let n = frames.len();
    let median = median_image(frames);
    let per_frame: Vec<(Vec<f32>, Vec<bool>)> = frames
        .iter()
        .map(|f| {
            let diff = compute_difference_map(f, &median);
            let mask = changed_mask(&diff, w, h, p);
            (diff, mask)
        })
        .collect();
    let len = (w * h) as usize;
    let labels: Vec<u8> = (0..len)
        .into_par_iter()
        .map(|i| {
            let clean = per_frame.iter().position(|(_, m)| !m[i]);
            clean
                .unwrap_or_else(|| (0..n).min_by(|&a, &b| per_frame[a].0[i].total_cmp(&per_frame[b].0[i])).unwrap_or(0))
                as u8
        })
        .collect();
    let weights: Vec<Vec<f32>> = (0..n)
        .map(|k| {
            let own: Vec<bool> = labels.iter().map(|&l| l as usize == k).collect();
            feather_alpha(&own, w, h, p.feather)
        })
        .collect();
    let alphas = sequential_alphas(&weights, w, h);
    let mut result = frames[0].clone();
    blend_sequence(&mut result, &frames[1..], &alphas);

    let any: Vec<bool> = (0..len).map(|i| per_frame.iter().any(|(_, m)| m[i])).collect();
    let changed = any.iter().filter(|&&m| m).count() as f32 / len.max(1) as f32;
    let mut mask_vis = RgbImage::new(w, h);
    for (i, px) in mask_vis.pixels_mut().enumerate() {
        px.0 = if any[i] { SOURCE_COLORS[labels[i] as usize % SOURCE_COLORS.len()] } else { [15, 23, 42] };
    }
    StackPreview { result, mask_vis, alphas, changed }
}

/// Карта различий: зона A — голубая, зона B — красная.
pub fn mask_visualization(mask: &[bool], w: u32, h: u32, angles: ClusterAngles) -> RgbImage {
    let wu = w as usize;
    let zone: Vec<bool> = (0..wu).map(|x| in_zone_a(x, wu, angles)).collect();
    let mut vis = RgbImage::new(w, h);
    for (i, px) in vis.pixels_mut().enumerate() {
        px.0 = if !mask[i] {
            [15, 23, 42]
        } else if zone[i % wu] {
            [56, 189, 248]
        } else {
            [239, 68, 68]
        };
    }
    vis
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute_dilate(mask: &[bool], w: u32, h: u32, r: i32) -> Vec<bool> {
        let mut out = vec![false; mask.len()];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                'o: for dy in -r..=r {
                    let ny = y + dy;
                    if ny < 0 || ny >= h as i32 {
                        continue;
                    }
                    for dx in -r..=r {
                        if dx * dx + dy * dy <= r * r {
                            let nx = (x + dx).rem_euclid(w as i32);
                            if mask[(ny * w as i32 + nx) as usize] {
                                out[(y * w as i32 + x) as usize] = true;
                                break 'o;
                            }
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn fast_dilation_matches_disk() {
        let (w, h) = (64u32, 32u32);
        let mut mask = vec![false; (w * h) as usize];
        for &(x, y) in &[(0, 5), (63, 20), (30, 0), (31, 31), (10, 15)] {
            mask[(y * w + x) as usize] = true;
        }
        for r in [1, 3, 7, 12] {
            assert_eq!(dilate_mask_360(&mask, w, h, r), brute_dilate(&mask, w, h, r), "r={r}");
        }
    }

    #[test]
    fn erosion_removes_specks() {
        let (w, h) = (40u32, 20u32);
        let mut mask = vec![false; (w * h) as usize];
        mask[(5 * w + 5) as usize] = true; // одиночная точка
        for y in 8..16 {
            for x in 20..30 {
                mask[(y * w + x) as usize] = true;
            }
        }
        let opened = dilate_mask_360(&erode_mask_360(&mask, w, h, 1), w, h, 1);
        assert!(!opened[(5 * w + 5) as usize]);
        assert!(opened[(12 * w + 25) as usize]);
    }

    #[test]
    fn circular_distance_wraps() {
        assert!((circular_distance(0.1, 2.0 * PI - 0.1) - 0.2).abs() < 1e-5);
        assert!((circular_distance(0.0, PI) - PI).abs() < 1e-5);
    }

    /// Фон — горизонтальный градиент; «человек» стоит на первом кадре в одном месте, на втором — в другом.
    fn scene(w: u32, h: u32, person_x: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            let px = (x as i64 - person_x as i64).rem_euclid(w as i64) as u32;
            if px < w / 20 && y > h / 3 && y < h * 3 / 4 {
                image::Rgb([200, 30, 40])
            } else {
                image::Rgb([(x * 255 / w) as u8, 120, (y * 255 / h) as u8])
            }
        })
    }

    fn clean(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, 120, (y * 255 / h) as u8]))
    }

    fn max_err(a: &RgbImage, b: &RgbImage) -> u8 {
        a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap()).max().unwrap()
    }

    #[test]
    fn removes_person_with_one_of_two_swaps() {
        let (w, h) = (400u32, 200u32);
        // второй человек «переходит» через шов панорамы
        let img1 = scene(w, h, 80);
        let img2 = scene(w, h, 390);
        let truth = clean(w, h);
        let p = Params { dilate: 6, feather: 3, ..Params::default() };
        let a = compute_preview(&img1, &img2, p);
        let b = compute_preview(&img1, &img2, Params { swap: true, ..p });
        let best = max_err(&a.result, &truth).min(max_err(&b.result, &truth));
        assert!(best <= 2, "person left in the result, max error {best}");
        // одна из зон около 80/400·360° ≈ 72°+, другая около шва
        let deg = |r: f32| r.to_degrees();
        let near = |a: f32, t: f32| circular_distance(a.to_radians(), t.to_radians()).to_degrees() < 15.0;
        let (za, zb) = (deg(a.angles.angle_a), deg(a.angles.angle_b));
        assert!((near(za, 81.0) && near(zb, 0.0)) || (near(zb, 81.0) && near(za, 0.0)), "{za} {zb}");
    }

    #[test]
    fn export_matches_preview() {
        let (w, h) = (400u32, 200u32);
        let img1 = scene(w, h, 80);
        let img2 = scene(w, h, 250);
        let small1 = downscale(img1.clone(), 200);
        let small2 = downscale(img2.clone(), 200);
        let p = Params { dilate: 4, feather: 2, ..Params::default() };
        let preview = compute_preview(&small1, &small2, p);
        let mut full = img1.clone();
        blend_into(&mut full, &img2, &preview.alpha);
        let truth = clean(w, h);
        let mut swapped = img1.clone();
        blend_into(&mut swapped, &img2, &compute_preview(&small1, &small2, Params { swap: true, ..p }).alpha);
        let best = max_err(&full, &truth).min(max_err(&swapped, &truth));
        assert!(best <= 3, "full-size export differs from preview logic, max error {best}");
    }

    #[test]
    fn stack_removes_passers_by() {
        let (w, h) = (400u32, 200u32);
        // на каждом кадре прохожий в своём месте, один — через шов панорамы
        let frames: Vec<RgbImage> = [60, 180, 300, 390].iter().map(|&x| scene(w, h, x)).collect();
        let p = Params { dilate: 6, feather: 3, ..Params::default() };
        let s = compute_stack_preview(&frames, p);
        let err = max_err(&s.result, &clean(w, h));
        assert!(err <= 3, "passers-by left in the result, max error {err}");
        assert!(s.changed > 0.0 && s.changed < 0.5, "changed {}", s.changed);

        // экспорт по растянутым маскам совпадает с превью
        let big: Vec<RgbImage> = [60, 180, 300, 390].iter().map(|&x| scene(w * 2, h * 2, x * 2)).collect();
        let small: Vec<RgbImage> = big.iter().map(|f| downscale(f.clone(), w)).collect();
        let s = compute_stack_preview(&small, p);
        let mut full = big[0].clone();
        blend_sequence(&mut full, &big[1..], &s.alphas);
        let err = max_err(&full, &clean(w * 2, h * 2));
        assert!(err <= 4, "full-size stack differs, max error {err}");
    }

    #[test]
    fn sequential_alphas_reproduce_weights() {
        let weights = vec![vec![0.5, 0.0, 0.2], vec![0.25, 0.0, 0.0], vec![0.25, 1.0, 0.8]];
        let alphas = sequential_alphas(&weights, 3, 1);
        // три «кадра» с яркостью 0, 100, 200 → итог Σ weight·яркость
        let frame = |v: u8| RgbImage::from_pixel(3, 1, image::Rgb([v, v, v]));
        let mut base = frame(0);
        blend_sequence(&mut base, &[frame(100), frame(200)], &alphas);
        let got: Vec<u8> = base.pixels().map(|p| p[0]).collect();
        assert_eq!(got, vec![75, 200, 160]);
    }

    #[test]
    fn median_of_frames() {
        let f = |v: u8| RgbImage::from_pixel(2, 1, image::Rgb([v, v, 255 - v]));
        assert_eq!(median_image(&[f(10), f(200), f(30)]).get_pixel(0, 0).0, [30, 30, 225]);
        assert_eq!(median_image(&[f(10), f(20), f(30), f(250)]).get_pixel(1, 0).0, [25, 25, 230]);
    }

    #[test]
    fn identical_frames_change_nothing() {
        let img = clean(200, 100);
        let p = compute_preview(&img, &img, Params::default());
        assert_eq!(p.changed, 0.0);
        assert_eq!(max_err(&p.result, &img), 0);
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    /// `cargo test --release -- --ignored --nocapture perf`: время расчёта превью и экспорта 8K.
    #[test]
    #[ignore]
    fn timings() {
        let make = |px: u32, w: u32, h: u32| {
            RgbImage::from_fn(w, h, |x, y| {
                let n = ((x.wrapping_mul(7919) ^ y.wrapping_mul(104729)) % 7) as u8; // шум
                if x.abs_diff(px) < w / 25 && y > h / 3 {
                    image::Rgb([200, 40, 40])
                } else {
                    image::Rgb([(x % 256) as u8 / 2 + n, 120 + n, (y % 256) as u8 / 2])
                }
            })
        };
        let (a, b) = (make(300, 1400, 700), make(900, 1400, 700));
        let t = std::time::Instant::now();
        let p = compute_preview(&a, &b, Params { dilate: 60, feather: 50, ..Params::default() });
        println!("preview 1400×700, r=60: {:?}", t.elapsed());
        let (mut fa, fb) = (make(1700, 8000, 4000), make(5100, 8000, 4000));
        let t = std::time::Instant::now();
        blend_into(&mut fa, &fb, &p.alpha);
        println!("blend 8000×4000: {:?}", t.elapsed());
    }
}

//! Сохранение панорамы в полном размере или уменьшенной: смешивание кадров по маскам превью, уменьшение,
//! JPEG с выбранным качеством, перенос метаданных 360° (размеры GPano пересчитываются). Вместо панорамы можно
//! сохранить кубическую карту: шесть файлов граней, полосу 6×1 или крест 4×3 (`cubemap`). Исходные фото тоже
//! могут быть кубической картой (`source`): открываются панорамой, а у сохранённой панорамы появляется GPano.

use crate::cubemap::{self, Cell, Face};
use crate::inpaint;
use crate::processor::{self, Alpha};
use crate::source::{self, Layout};
use crate::xmp;
use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::RgbImage;
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use std::path::{Path, PathBuf};

/// Ширины для выбора в окне, по убыванию. 0 — исходный размер.
pub const WIDTHS: [u32; 5] = [8192, 6144, 5120, 4096, 2048];

/// Что сохранять. В настройках и окне — строкой (`name`); незнакомая строка — панорама.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Format {
    /// Панорама 2:1, как исходные фото.
    #[default]
    Equirect,
    /// Шесть файлов граней `_f _r _b _l _u _d`.
    CubeFiles,
    /// Один файл: грани полосой 6×1.
    CubeStrip,
    /// Один файл: грани крестом 4×3.
    CubeCross,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Equirect => "equirect",
            Format::CubeFiles => "cube",
            Format::CubeStrip => "strip",
            Format::CubeCross => "cross",
        }
    }

    /// Формат, в каком было открытое фото.
    pub fn of(layout: Layout) -> Self {
        match layout {
            Layout::Equirect => Format::Equirect,
            Layout::Strip => Format::CubeStrip,
            Layout::Cross => Format::CubeCross,
            Layout::Faces => Format::CubeFiles,
        }
    }

    /// Окончание имени файла по умолчанию.
    pub fn tag(self) -> &'static str {
        match self {
            Format::Equirect => "",
            Format::CubeFiles => "_cube",
            Format::CubeStrip => "_cubestrip",
            Format::CubeCross => "_cubecross",
        }
    }

    /// Расположение граней в файле; у панорамы и отдельных файлов — нет.
    fn cells(self) -> Option<&'static [Cell]> {
        match self {
            Format::CubeStrip => Some(&cubemap::STRIP),
            Format::CubeCross => Some(&cubemap::CROSS),
            _ => None,
        }
    }
}

impl From<String> for Format {
    fn from(s: String) -> Self {
        [Format::CubeFiles, Format::CubeStrip, Format::CubeCross]
            .into_iter()
            .find(|f| f.name() == s)
            .unwrap_or_default()
    }
}

impl From<Format> for String {
    fn from(f: Format) -> Self {
        f.name().to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Ширина панорамы; 0 или не меньше исходной — исходный размер (увеличения нет). У кубической карты сторона
    /// грани — четверть этой ширины.
    pub width: u32,
    /// Качество JPEG 1–100.
    pub quality: u8,
    pub format: Format,
}

/// Файлы, которые запишет сохранение в `out`: сам `out`, а у отдельных граней — `имя_f.jpg` … `имя_d.jpg` рядом.
pub fn output_paths(out: &Path, format: Format) -> Vec<PathBuf> {
    if format != Format::CubeFiles {
        return vec![out.to_path_buf()];
    }
    let stem = out.file_stem().unwrap_or_default().to_string_lossy();
    Face::ALL.iter().map(|f| out.with_file_name(format!("{stem}_{}.jpg", f.suffix()))).collect()
}

/// Сначала во временный файл рядом, потом переименование: при сбое старый файл не испортится.
fn write_file(out: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = out.with_extension("jpg.part");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Размер результата для кадра `w`×`h`: пропорции сохраняются.
pub fn target_dims(w: u32, h: u32, width: u32) -> (u32, u32) {
    if width == 0 || width >= w {
        return (w, h);
    }
    let th = ((h as u64 * width as u64 + w as u64 / 2) / w as u64).max(1) as u32;
    (width, th)
}

/// Уменьшение Lanczos3 (SIMD, все ядра). Тот же размер — без изменений.
pub fn resize_to(img: RgbImage, w: u32, h: u32) -> Result<RgbImage, String> {
    if img.dimensions() == (w, h) {
        return Ok(img);
    }
    let (sw, sh) = img.dimensions();
    let src = Image::from_vec_u8(sw, sh, img.into_raw(), PixelType::U8x3).map_err(|e| e.to_string())?;
    let mut dst = Image::new(w, h, PixelType::U8x3);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new().resize(&src, &mut dst, &options).map_err(|e| e.to_string())?;
    RgbImage::from_raw(w, h, dst.into_vec()).ok_or_else(|| "resize: bad buffer".to_string())
}

/// JPEG для сохранения: оптимизированные таблицы Хаффмана и прогрессивная запись (файл меньше при том же
/// качестве); цвет 4:2:0, при качестве от 95 — без уменьшения цвета (4:4:4).
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let (w, h) = img.dimensions();
    let (Ok(w16), Ok(h16)) = (u16::try_from(w), u16::try_from(h)) else {
        return Err(format!("{w}×{h} is too large for JPEG (max 65535)"));
    };
    let mut buf = Vec::new();
    let mut encoder = Encoder::new(&mut buf, quality.clamp(1, 100));
    encoder.set_sampling_factor(if quality >= 95 { SamplingFactor::R_4_4_4 } else { SamplingFactor::R_4_2_0 });
    encoder.set_optimized_huffman_tables(true);
    encoder.set_progressive(true);
    encoder.encode(img.as_raw(), w16, h16, ColorType::Rgb).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Итог сохранения: размер одного файла и вес всех вместе.
#[derive(Debug)]
pub struct Saved {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub files: Vec<PathBuf>,
}

/// Кадр 0 — основа (его размер и метаданные), кадры 1.. смешиваются в него по маскам `alphas` (по одной на кадр).
/// В памяти одновременно только два кадра.
/// Результат шириной `width` (0 — исходная): кадры по маскам `alphas`. Второе — исходный размер первого кадра
/// (панорамы, в которую он открылся), третье — в каком виде он был.
pub fn compose(paths: &[PathBuf], alphas: &[Alpha], width: u32) -> Result<(RgbImage, (u32, u32), Layout), String> {
    let first = paths.first().ok_or("no photos")?;
    let loaded = source::load(first, None)?;
    let (base, layout) = (loaded.image, loaded.layout);
    let (w0, h0) = base.dimensions();
    let (w, h) = target_dims(w0, h0, width);
    let mut base = resize_to(base, w, h)?;
    for (path, alpha) in paths[1..].iter().zip(alphas) {
        // кадр, из которого ничего не берётся, незачем открывать
        if alpha.data.iter().all(|&a| a <= 0.0) {
            continue;
        }
        let other = resize_to(processor::load_image(path, None)?, w, h)?;
        processor::blend_into(&mut base, &other, alpha);
    }
    Ok((base, (w0, h0), layout))
}

/// `patches` — куски ИИ-дорисовки (пусто, если её нет): ложатся поверх склеенных кадров.
pub fn export(
    paths: &[PathBuf],
    alphas: &[Alpha],
    patches: &[inpaint::Patch],
    options: Options,
    out: &Path,
) -> Result<Saved, String> {
    let first = paths.first().ok_or("no photos")?;
    let (mut base, (w0, h0), layout) = compose(paths, alphas, options.width)?;
    let (w, h) = base.dimensions();
    for patch in patches {
        patch.apply(&mut base);
    }
    let markers = std::fs::read(first).map(|b| xmp::extract_jpeg_markers(&b)).unwrap_or_default();
    let files = output_paths(out, options.format);
    if options.format == Format::Equirect {
        let jpeg = encode_jpeg(&base, options.quality)?;
        drop(base);
        let bytes = xmp::inject_markers(&jpeg, &panorama_markers(markers, layout, (w0, h0), (w, h)));
        write_file(out, &bytes)?;
        return Ok(Saved { width: w, height: h, bytes: bytes.len() as u64, files });
    }
    // грани — уже не сферическая панорама: GPano им не нужен, EXIF и цветовой профиль остаются
    let markers = xmp::without_xmp(markers);
    let n = cubemap::face_size(w);
    let mut total = 0u64;
    let (fw, fh) = match options.format.cells() {
        Some(cells) => {
            let img = cubemap::render(&base, n, cells);
            drop(base);
            let bytes = xmp::inject_markers(&encode_jpeg(&img, options.quality)?, &markers);
            write_file(out, &bytes)?;
            total += bytes.len() as u64;
            img.dimensions()
        }
        None => {
            for (face, path) in Face::ALL.iter().zip(&files) {
                let img = cubemap::render_face(&base, n, *face);
                let bytes = xmp::inject_markers(&encode_jpeg(&img, options.quality)?, &markers);
                write_file(path, &bytes)?;
                total += bytes.len() as u64;
            }
            (n, n)
        }
    };
    Ok(Saved { width: fw, height: fh, bytes: total, files })
}

/// Метаданные панорамы `to` из метаданных первого фото размером `from`: GPano пересчитан, а у кубической карты
/// его не было — пишется новый.
fn panorama_markers(markers: Vec<Vec<u8>>, layout: Layout, from: (u32, u32), to: (u32, u32)) -> Vec<Vec<u8>> {
    if !layout.is_cube() {
        return xmp::scale_gpano(markers, from, to);
    }
    let mut markers = xmp::without_xmp(markers);
    markers.push(xmp::gpano(to.0, to.1));
    markers
}

/// Сколько пикселей результата сжимается для оценки веса: больше — точнее, но дольше.
const ESTIMATE_PIXELS: u64 = 2_000_000;
/// Сторона куска выборки: кратна 16, так что блоки JPEG (и цвета при 4:2:0) не ложатся на стык кусков.
const ESTIMATE_TILE: u32 = 128;

/// Выборка для оценки веса: результат целиком, если он небольшой, иначе куски `ESTIMATE_TILE` равномерной
/// сеткой по всей панораме, сложенные в одну картинку. Второе число — во сколько раз результат больше выборки.
pub fn estimate_sample(img: &RgbImage) -> (RgbImage, f64) {
    let (w, h) = img.dimensions();
    sample_tiles(w, h, |x, y, tw, th| image::imageops::crop_imm(img, x, y, tw, th).to_image())
}

/// То же для картинки `w`×`h`, которой ещё нет: `tile(x, y, ширина, высота)` даёт её кусок.
fn sample_tiles(w: u32, h: u32, tile: impl Fn(u32, u32, u32, u32) -> RgbImage) -> (RgbImage, f64) {
    let total = w as u64 * h as u64;
    let t = ESTIMATE_TILE;
    if total <= ESTIMATE_PIXELS * 2 || w < t * 4 || h < t * 2 {
        return (tile(0, 0, w, h), 1.0);
    }
    // сетка с теми же пропорциями, что у картинки
    let tiles = (ESTIMATE_PIXELS / (t as u64 * t as u64)).max(8) as f64;
    let cols = ((tiles * w as f64 / h as f64).sqrt().round() as u32).clamp(2, w / t);
    let rows = ((tiles / cols as f64).round() as u32).clamp(1, h / t);
    let mut sample = RgbImage::new(cols * t, rows * t);
    for r in 0..rows {
        for c in 0..cols {
            // середина клетки сетки, выровненная по 16
            let x =
                ((((2 * c + 1) as u64 * w as u64 / (2 * cols as u64)) as u32).saturating_sub(t / 2) & !15).min(w - t);
            let y =
                ((((2 * r + 1) as u64 * h as u64 / (2 * rows as u64)) as u32).saturating_sub(t / 2) & !15).min(h - t);
            image::imageops::replace(&mut sample, &tile(x, y, t, t), (c * t) as i64, (r * t) as i64);
        }
    }
    let scale = total as f64 / (sample.width() as u64 * sample.height() as u64) as f64;
    (sample, scale)
}

/// Примерный вес сохранённого (у граней — всех файлов вместе) при каждом качестве из `qualities`: первое фото
/// в размере результата, сжатое так же, как при сохранении (по выборке), плюс метаданные. Люди на фото на вес
/// почти не влияют. Грани для выборки — в той же раскладке (у отдельных файлов — полосой): пустые клетки креста
/// тоже весят.
pub fn estimate_sizes(first: &Path, width: u32, format: Format, qualities: &[u8]) -> Result<Vec<(u8, u64)>, String> {
    use rayon::prelude::*;
    let loaded = source::load(first, None)?;
    let (img, layout) = (loaded.image, loaded.layout);
    let (w0, h0) = img.dimensions();
    let (w, h) = target_dims(w0, h0, width);
    let img = resize_to(img, w, h)?;
    let markers = std::fs::read(first).map(|b| xmp::extract_jpeg_markers(&b)).unwrap_or_default();
    let (sample, scale, markers, files) = if format == Format::Equirect {
        let (sample, scale) = estimate_sample(&img);
        (sample, scale, panorama_markers(markers, layout, (w0, h0), (w, h)), 1)
    } else {
        let n = cubemap::face_size(w);
        let cells = format.cells().unwrap_or(&cubemap::STRIP);
        let (sw, sh) = cubemap::layout_dims(n, cells);
        let (sample, scale) = sample_tiles(sw, sh, |x, y, tw, th| cubemap::render_region(&img, n, cells, x, y, tw, th));
        let files = if format == Format::CubeFiles { Face::ALL.len() as u64 } else { 1 };
        (sample, scale, xmp::without_xmp(markers), files)
    };
    drop(img);
    let meta: u64 = markers.iter().map(|m| m.len() as u64).sum::<u64>() * files;
    qualities
        .par_iter()
        .map(|&q| encode_jpeg(&sample, q).map(|j| (q, (j.len() as f64 * scale).round() as u64 + meta)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processor::Params;
    use image::codecs::jpeg::JpegEncoder;

    fn jpeg(img: &RgbImage) -> Vec<u8> {
        let mut buf = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, 95).encode_image(img).unwrap();
        buf
    }

    fn gpano(w: u32, h: u32) -> Vec<u8> {
        let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        payload.extend(format!("<x GPano:FullPanoWidthPixels=\"{w}\" GPano:FullPanoHeightPixels=\"{h}\"/>").as_bytes());
        let len = payload.len() + 2;
        let mut seg = vec![0xFF, 0xE1, (len >> 8) as u8, len as u8];
        seg.extend(payload);
        seg
    }

    fn frame(w: u32, h: u32, px: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            if x.abs_diff(px) < w / 40 && y > h * 3 / 8 && y < h * 3 / 4 {
                image::Rgb([220, 30, 30])
            } else {
                image::Rgb([(x * 255 / w) as u8, 110, (y * 255 / h) as u8])
            }
        })
    }

    fn red_count(img: &RgbImage, x: u32) -> usize {
        let (_, h) = img.dimensions();
        (h * 2 / 5..h * 7 / 10).filter(|&y| img.get_pixel(x, y)[0] > 200 && img.get_pixel(x, y)[1] < 60).count()
    }

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("cleaner360_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    #[test]
    fn target_sizes() {
        assert_eq!(target_dims(15520, 7760, 0), (15520, 7760));
        assert_eq!(target_dims(15520, 7760, 8192), (8192, 4096));
        assert_eq!(target_dims(15520, 7760, 20000), (15520, 7760));
        assert_eq!(target_dims(5760, 2880, 4096), (4096, 2048));
        assert_eq!(target_dims(1000, 333, 500), (500, 167));
    }

    #[test]
    fn pair_export_keeps_size_and_360_metadata() {
        let dir = temp_dir("pair");
        let (w, h) = (800u32, 400u32);
        let (a, b, out) = (dir.0.join("a.jpg"), dir.0.join("b.jpg"), dir.0.join("out.jpg"));
        std::fs::write(&a, xmp::inject_markers(&jpeg(&frame(w, h, 200)), &[gpano(w, h)])).unwrap();
        std::fs::write(&b, jpeg(&frame(w, h, 600))).unwrap();

        let small = |p: &Path| processor::load_image(p, Some(400)).unwrap();
        let p = Params { dilate: 6, feather: 3, ..Default::default() };
        let preview = processor::compute_preview(&small(&a), &small(&b), p);
        let saved =
            export(&[a, b], &[preview.alpha], &[], Options { width: 0, quality: 95, format: Format::Equirect }, &out)
                .unwrap();
        assert_eq!((saved.width, saved.height), (w, h));

        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(bytes.len() as u64, saved.bytes);
        assert_eq!(xmp::extract_jpeg_markers(&bytes), vec![gpano(w, h)]);
        let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (w, h));
        // одна из двух зон без человека: в его месте нет ярко-красного
        assert!(red_count(&img, 200) == 0 || red_count(&img, 600) == 0, "both people are still there");
        assert!(!dir.0.join("out.jpg.part").exists());
    }

    #[test]
    fn single_photo_resaved_smaller() {
        let dir = temp_dir("single");
        let (w, h) = (1600u32, 800u32);
        let (a, out) = (dir.0.join("a.jpg"), dir.0.join("out.jpg"));
        std::fs::write(&a, xmp::inject_markers(&jpeg(&frame(w, h, 300)), &[gpano(w, h)])).unwrap();
        let saved = export(
            std::slice::from_ref(&a),
            &[],
            &[],
            Options { width: 800, quality: 80, format: Format::Equirect },
            &out,
        )
        .unwrap();
        assert_eq!((saved.width, saved.height), (800, 400));
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(xmp::extract_jpeg_markers(&bytes), vec![gpano(800, 400)]);
        assert!(bytes.len() < std::fs::metadata(&a).unwrap().len() as usize);
        let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (800, 400));
        assert!(red_count(&img, 150) > 0, "content lost on resize");
    }

    #[test]
    fn sample_is_whole_small_image_or_tiles_of_large() {
        let small = frame(1600, 800, 300);
        let (s, k) = estimate_sample(&small);
        assert_eq!((s.dimensions(), k), ((1600, 800), 1.0));
        let big = frame(8192, 4096, 2000);
        let (s, k) = estimate_sample(&big);
        let px = s.width() as u64 * s.height() as u64;
        assert!(px > 1_500_000 && px < 2_600_000, "sample {}×{}", s.width(), s.height());
        assert!((k - 8192.0 * 4096.0 / px as f64).abs() < 1e-9);
        assert_eq!((s.width() % ESTIMATE_TILE, s.height() % ESTIMATE_TILE), (0, 0));
    }

    #[test]
    fn estimate_close_to_saved_size() {
        let dir = temp_dir("estimate");
        let (w, h) = (6000u32, 3000u32);
        // мелкий узор, чтобы вес зависел от качества
        let img = RgbImage::from_fn(w, h, |x, y| {
            let v = ((x * 7 + y * 13) ^ (x / 3 * y / 5)) as u8;
            image::Rgb([v, v.wrapping_mul(3), (x / 20) as u8])
        });
        let (a, out) = (dir.0.join("a.jpg"), dir.0.join("out.jpg"));
        std::fs::write(&a, jpeg(&img)).unwrap();
        let est = estimate_sizes(&a, 4096, Format::Equirect, &[60, 85, 95]).unwrap();
        for (q, bytes) in est {
            let saved = export(
                std::slice::from_ref(&a),
                &[],
                &[],
                Options { width: 4096, quality: q, format: Format::Equirect },
                &out,
            )
            .unwrap();
            let err = bytes as f64 / saved.bytes as f64 - 1.0;
            assert!(err.abs() < 0.1, "q{q}: estimate {bytes}, saved {}", saved.bytes);
        }
    }

    #[test]
    fn cubemap_faces_and_layouts() {
        let dir = temp_dir("cube");
        let (w, h) = (1600u32, 800u32);
        let a = dir.0.join("a.jpg");
        let exif = {
            let mut s = vec![0xFF, 0xE1, 0, 10];
            s.extend(b"Exif\0\0MM");
            s
        };
        std::fs::write(&a, xmp::inject_markers(&jpeg(&frame(w, h, 300)), &[exif.clone(), gpano(w, h)])).unwrap();
        let options = |format| Options { width: 0, quality: 85, format };

        let out = dir.0.join("pano.jpg");
        let saved = export(std::slice::from_ref(&a), &[], &[], options(Format::CubeFiles), &out).unwrap();
        assert_eq!((saved.width, saved.height), (400, 400));
        let names: Vec<String> = saved.files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect();
        assert_eq!(names, ["pano_f.jpg", "pano_r.jpg", "pano_b.jpg", "pano_l.jpg", "pano_u.jpg", "pano_d.jpg"]);
        assert!(!out.exists());
        let mut total = 0;
        for f in &saved.files {
            let bytes = std::fs::read(f).unwrap();
            total += bytes.len() as u64;
            // EXIF остаётся, GPano — нет: грань не панорама
            assert_eq!(xmp::extract_jpeg_markers(&bytes), vec![exif.clone()]);
            assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgb8().dimensions(), (400, 400));
        }
        assert_eq!(total, saved.bytes);

        let strip = dir.0.join("strip.jpg");
        let saved = export(std::slice::from_ref(&a), &[], &[], options(Format::CubeStrip), &strip).unwrap();
        assert_eq!((saved.width, saved.height, saved.files.len()), (2400, 400, 1));
        assert_eq!(image::open(&strip).unwrap().to_rgb8().dimensions(), (2400, 400));
        let cross = dir.0.join("cross.jpg");
        let saved = export(std::slice::from_ref(&a), &[], &[], options(Format::CubeCross), &cross).unwrap();
        assert_eq!((saved.width, saved.height), (1600, 1200));
    }

    #[test]
    fn cubemap_photo_saved_as_panorama() {
        let dir = temp_dir("cube_in");
        let a = dir.0.join("a.jpg");
        std::fs::write(&a, jpeg(&frame(1600, 800, 300))).unwrap();
        let strip = dir.0.join("a_strip.jpg");
        let options = |format| Options { width: 0, quality: 90, format };
        export(std::slice::from_ref(&a), &[], &[], options(Format::CubeStrip), &strip).unwrap();
        // полоса обратно в панораму: тот же размер, и у файла теперь есть GPano
        let out = dir.0.join("back.jpg");
        let saved = export(std::slice::from_ref(&strip), &[], &[], options(Format::Equirect), &out).unwrap();
        assert_eq!((saved.width, saved.height), (1600, 800));
        let bytes = std::fs::read(&out).unwrap();
        let markers = xmp::extract_jpeg_markers(&bytes);
        assert!(markers.iter().any(|m| String::from_utf8_lossy(m).contains("GPano:FullPanoWidthPixels=\"1600\"")));
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert!(red_count(&back, 300) > 100);
        // и снова в полосу: того же размера, что была
        let again = dir.0.join("again.jpg");
        let saved = export(std::slice::from_ref(&strip), &[], &[], options(Format::CubeStrip), &again).unwrap();
        assert_eq!((saved.width, saved.height), (2400, 400));
    }

    #[test]
    fn cubemap_estimate_close_to_saved_size() {
        let dir = temp_dir("cube_estimate");
        let (w, h) = (6000u32, 3000u32);
        let img = RgbImage::from_fn(w, h, |x, y| {
            let v = ((x * 7 + y * 13) ^ (x / 3 * y / 5)) as u8;
            image::Rgb([v, v.wrapping_mul(3), (x / 20) as u8])
        });
        let (a, out) = (dir.0.join("a.jpg"), dir.0.join("out.jpg"));
        std::fs::write(&a, jpeg(&img)).unwrap();
        for format in [Format::CubeFiles, Format::CubeCross] {
            let est = estimate_sizes(&a, 0, format, &[85]).unwrap()[0].1;
            let saved =
                export(std::slice::from_ref(&a), &[], &[], Options { width: 0, quality: 85, format }, &out).unwrap();
            let err = est as f64 / saved.bytes as f64 - 1.0;
            assert!(err.abs() < 0.1, "{format:?}: estimate {est}, saved {}", saved.bytes);
        }
    }

    /// `CLEANER360_SAMPLE=photo.jpg cargo test -- --ignored cross_preview`: крест 4×3 из настоящей панорамы рядом с
    /// ней, чтобы посмотреть швы глазами.
    #[test]
    #[ignore]
    fn cross_preview() {
        let Some(src) = std::env::var_os("CLEANER360_SAMPLE").map(PathBuf::from) else { return };
        let out = std::env::temp_dir().join("cleaner360_cross.jpg");
        export(&[src], &[], &[], Options { width: 4096, quality: 85, format: Format::CubeCross }, &out).unwrap();
        println!("{}", out.display());
    }

    #[test]
    fn format_names_round_trip() {
        for f in [Format::Equirect, Format::CubeFiles, Format::CubeStrip, Format::CubeCross] {
            assert_eq!(Format::from(String::from(f)), f);
        }
        assert_eq!(Format::from("nonsense".to_string()), Format::Equirect);
    }

    #[test]
    fn stack_export_removes_passers_by() {
        let dir = temp_dir("stack");
        let (w, h) = (800u32, 400u32);
        let paths: Vec<PathBuf> = [100u32, 300, 500, 700]
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let p = dir.0.join(format!("{i}.jpg"));
                std::fs::write(&p, jpeg(&frame(w, h, x))).unwrap();
                p
            })
            .collect();
        let small: Vec<RgbImage> = paths.iter().map(|p| processor::load_image(p, Some(400)).unwrap()).collect();
        let s = processor::compute_stack_preview(&small, Params { dilate: 6, feather: 3, ..Default::default() });
        let out = dir.0.join("out.jpg");
        export(&paths, &s.alphas, &[], Options { width: 0, quality: 90, format: Format::Equirect }, &out).unwrap();
        let img = image::open(&out).unwrap().to_rgb8();
        for x in [100, 300, 500, 700] {
            assert_eq!(red_count(&img, x), 0, "passer-by left at x={x}");
        }
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    /// `CLEANER360_SAMPLE=photo.jpg cargo test --release -- --ignored --nocapture save_sizes`: время сохранения и
    /// вес файла настоящей панорамы при разных размерах и качестве.
    #[test]
    #[ignore]
    fn save_sizes() {
        let Some(src) = std::env::var_os("CLEANER360_SAMPLE").map(PathBuf::from) else { return };
        let out = std::env::temp_dir().join(format!("cleaner360_sizes_{}.jpg", std::process::id()));
        let runs =
            [(0, 95), (0, 85), (8192, 85), (6144, 85), (5120, 80), (4096, 80)].map(|(w, q)| (w, q, Format::Equirect));
        let cube = [(0, 85, Format::CubeFiles), (8192, 85, Format::CubeFiles), (0, 85, Format::CubeCross)];
        for (width, quality, format) in runs.into_iter().chain(cube) {
            let t = std::time::Instant::now();
            let s = export(std::slice::from_ref(&src), &[], &[], Options { width, quality, format }, &out).unwrap();
            let mb = s.bytes as f64 / 1_048_576.0;
            println!("{format:?} {}×{} q{quality}: {mb:.2} MB, {:?}", s.width, s.height, t.elapsed());
            let t = std::time::Instant::now();
            let est = estimate_sizes(&src, width, format, &[quality]).unwrap()[0].1;
            println!(
                "  estimate {:.2} MB ({:+.1}%), {:?}",
                est as f64 / 1_048_576.0,
                (est as f64 / s.bytes as f64 - 1.0) * 100.0,
                t.elapsed()
            );
        }
        let t = std::time::Instant::now();
        let all: Vec<u8> = (crate::settings::MIN_QUALITY..=100).collect();
        estimate_sizes(&src, 0, Format::Equirect, &all).unwrap();
        println!("estimate for q{}–100 at full size: {:?}", all[0], t.elapsed());
        for f in output_paths(&out, Format::CubeFiles) {
            let _ = std::fs::remove_file(f);
        }
        let _ = std::fs::remove_file(out);
    }
}

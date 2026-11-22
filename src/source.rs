//! Открытие снимка в любом виде, в каком программа умеет сохранять: панорама 2:1, кубическая карта полосой 6×1,
//! крестом 4×3 или шестью файлами граней `имя_f.jpg` … `имя_d.jpg`. Кубическая карта сразу переводится в
//! панораму (`cubemap::to_equirect`), и дальше всё идёт как с обычной панорамой.
//!
//! Снимок из шести файлов в списке — один, путь у него — передней грани (`canonical`).

use crate::cubemap::{self, Face};
use crate::processor;
use image::RgbImage;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Панорама (или что-то незнакомое — тоже открывается как есть).
    Equirect,
    /// Полоса 6×1 (`cubemap::STRIP`).
    Strip,
    /// Крест 4×3 (`cubemap::CROSS`), пустые клетки чёрные.
    Cross,
    /// Шесть файлов граней.
    Faces,
}

impl Layout {
    pub fn is_cube(self) -> bool {
        self != Layout::Equirect
    }

    /// Подпись в окне.
    pub fn label(self) -> &'static str {
        match self {
            Layout::Equirect => "",
            Layout::Strip => "6×1",
            Layout::Cross => "4×3",
            Layout::Faces => "_f … _d",
        }
    }
}

/// Открытый снимок.
pub struct Loaded {
    /// Панорама; с `max_dim` — уменьшенная.
    pub image: RgbImage,
    pub layout: Layout,
    /// Размер панорамы в полном размере (у кубической карты — 4n×2n при стороне грани n).
    pub dims: (u32, u32),
    /// Размер файла (у граней — одной грани).
    pub file_dims: (u32, u32),
}

/// Окончание имени грани: буква после последнего `_` (любого регистра) и грань.
fn face_suffix(stem: &str) -> Option<(&str, char, Face)> {
    let (base, tail) = stem.rsplit_once('_')?;
    let mut chars = tail.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    let face = Face::ALL.into_iter().find(|f| f.suffix().eq_ignore_ascii_case(&c.to_string()))?;
    Some((base, c, face))
}

/// Шесть файлов граней (порядок `Face::ALL`), если `path` — одна из граней `имя_f.jpg` … `имя_d.jpg`, все шесть
/// лежат рядом и это квадраты одного размера. Буквы — того же регистра, что у `path`.
pub fn face_files(path: &Path) -> Option<[PathBuf; 6]> {
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?;
    let (base, c, _) = face_suffix(stem)?;
    let files = Face::ALL.map(|f| {
        let s = if c.is_ascii_uppercase() { f.suffix().to_ascii_uppercase() } else { f.suffix().to_string() };
        path.with_file_name(format!("{base}_{s}.{ext}"))
    });
    let first = image::image_dimensions(&files[0]).ok()?;
    let same = files.iter().all(|f| image::image_dimensions(f).ok() == Some(first));
    (first.0 == first.1 && same).then_some(files)
}

/// Путь снимка в списке: у граней — передней, у остального — сам `path`.
pub fn canonical(path: &Path) -> PathBuf {
    face_files(path).map_or_else(|| path.to_path_buf(), |[front, ..]| front)
}

/// Все файлы снимка: шесть граней или один файл.
pub fn files(path: &Path) -> Vec<PathBuf> {
    face_files(path).map_or_else(|| vec![path.to_path_buf()], Vec::from)
}

/// Основа имени для сохранения: у граней — без `_f`.
pub fn stem(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    match face_files(path).and(face_suffix(&stem)) {
        Some((base, _, _)) => base.to_string(),
        None => stem,
    }
}

/// Имя снимка в окне: у граней — `имя_*.jpg`.
pub fn display_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    if face_files(path).is_none() {
        return name;
    }
    let ext = path.extension().unwrap_or_default().to_string_lossy();
    format!("{}_*.{ext}", stem(path))
}

fn open(path: &Path) -> Result<RgbImage, String> {
    image::open(path).map(|i| i.to_rgb8()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Крест 4×3: размер делится на клетки, и все шесть пустых клеток почти чёрные (у обычного фото 4:3 так не бывает).
fn is_cross(img: &RgbImage) -> bool {
    let (w, h) = img.dimensions();
    if w % 4 != 0 || h % 3 != 0 || w / 4 != h / 3 || w < 4 {
        return false;
    }
    let n = w / 4;
    let used: Vec<(u32, u32)> = cubemap::CROSS.iter().map(|c| (c.1, c.2)).collect();
    // края клеток пропускаются: туда JPEG размазывает соседние грани
    let (m, step) = (n / 8, (n / 64).max(1));
    (0..3).flat_map(|r| (0..4).map(move |c| (c, r))).filter(|cell| !used.contains(cell)).all(|(c, r)| {
        let (mut sum, mut count) = (0u64, 0u64);
        for y in (r * n + m..(r + 1) * n - m).step_by(step as usize) {
            for x in (c * n + m..(c + 1) * n - m).step_by(step as usize) {
                sum += *img.get_pixel(x, y).0.iter().max().unwrap_or(&0) as u64;
                count += 1;
            }
        }
        count > 0 && sum < 12 * count
    })
}

/// Грани до стороны `n` (уменьшение для превью).
fn shrink(faces: [RgbImage; 6], n: u32) -> [RgbImage; 6] {
    if faces[0].width() <= n {
        return faces;
    }
    faces.map(|f| image::imageops::resize(&f, n, n, image::imageops::FilterType::Triangle))
}

/// Открывает снимок и приводит к панораме; с `max_dim` — не шире `max_dim` (у панорамы — большая сторона).
pub fn load(path: &Path, max_dim: Option<u32>) -> Result<Loaded, String> {
    let (layout, faces, file_dims) = match face_files(path) {
        Some(files) => {
            let faces: Vec<RgbImage> = files.par_iter().map(|f| open(f)).collect::<Result<_, _>>()?;
            let faces: [RgbImage; 6] = faces.try_into().map_err(|_| "six faces expected".to_string())?;
            let dims = faces[0].dimensions();
            if faces.iter().any(|f| f.dimensions() != dims) {
                return Err("cube faces differ in size".into());
            }
            (Layout::Faces, faces, dims)
        }
        None => {
            let img = open(path)?;
            let (w, h) = img.dimensions();
            let layout = if h > 0 && w == 6 * h {
                Layout::Strip
            } else if is_cross(&img) {
                Layout::Cross
            } else {
                let image = match max_dim {
                    Some(dim) => processor::downscale(img, dim),
                    None => img,
                };
                return Ok(Loaded { image, layout: Layout::Equirect, dims: (w, h), file_dims: (w, h) });
            };
            let cells = if layout == Layout::Strip { &cubemap::STRIP[..] } else { &cubemap::CROSS[..] };
            (layout, cubemap::split_layout(&img, w / if layout == Layout::Strip { 6 } else { 4 }, cells), (w, h))
        }
    };
    let n = faces[0].width();
    let faces = match max_dim {
        Some(dim) => shrink(faces, (dim / 4).max(1)),
        None => faces,
    };
    Ok(Loaded { image: cubemap::to_equirect(&faces), layout, dims: (4 * n, 2 * n), file_dims })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panorama(w: u32) -> RgbImage {
        RgbImage::from_fn(w, w / 2, |x, y| image::Rgb([(x * 256 / w) as u8, 128, (255 - y * 256 / (w / 2)) as u8]))
    }

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("cleaner360_src_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn close(a: &RgbImage, b: &RgbImage) {
        assert_eq!(a.dimensions(), b.dimensions());
        let (w, h) = a.dimensions();
        for y in (h / 10..h * 9 / 10).step_by(5) {
            for x in (8..w - 8).step_by(5) {
                let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
                assert!((0..3).all(|c| (p[c] as i32 - q[c] as i32).abs() <= 3), "{x},{y}: {p:?} vs {q:?}");
            }
        }
    }

    #[test]
    fn layouts_open_as_panorama() {
        let dir = temp_dir("layouts");
        let eq = panorama(800);
        let n = cubemap::face_size(800);
        for (cells, layout, name) in
            [(&cubemap::STRIP[..], Layout::Strip, "s.png"), (&cubemap::CROSS[..], Layout::Cross, "c.png")]
        {
            let path = dir.0.join(name);
            cubemap::render(&eq, n, cells).save(&path).unwrap();
            let full = load(&path, None).unwrap();
            assert_eq!((full.layout, full.dims), (layout, (800, 400)));
            close(&full.image, &eq);
            let small = load(&path, Some(400)).unwrap();
            assert_eq!((small.image.dimensions(), small.dims), ((400, 200), (800, 400)));
            assert_eq!(canonical(&path), path);
        }
        // обычное фото 4:3 — не крест
        let photo = dir.0.join("photo.png");
        RgbImage::from_pixel(400, 300, image::Rgb([90, 120, 60])).save(&photo).unwrap();
        assert_eq!(load(&photo, None).unwrap().layout, Layout::Equirect);
    }

    #[test]
    fn six_files_are_one_photo() {
        let dir = temp_dir("faces");
        let eq = panorama(800);
        let n = cubemap::face_size(800);
        for face in Face::ALL {
            cubemap::render_face(&eq, n, face).save(dir.0.join(format!("pano_{}.png", face.suffix()))).unwrap();
        }
        let right = dir.0.join("pano_r.png");
        let front = dir.0.join("pano_f.png");
        assert_eq!(canonical(&right), front);
        assert_eq!(files(&right).len(), 6);
        assert_eq!(stem(&front), "pano");
        assert_eq!(display_name(&front), "pano_*.png");
        let full = load(&right, None).unwrap();
        assert_eq!((full.layout, full.dims, full.file_dims), (Layout::Faces, (800, 400), (200, 200)));
        close(&full.image, &eq);
        // без одной грани — просто квадратное фото
        std::fs::remove_file(dir.0.join("pano_d.png")).unwrap();
        assert_eq!(canonical(&right), right);
        assert_eq!(stem(&front), "pano_f");
        assert_eq!(load(&right, None).unwrap().layout, Layout::Equirect);
    }

    /// `cargo test --release -- --ignored --nocapture cube_timings`: сколько открывается полоса с гранью 2048.
    #[test]
    #[ignore]
    fn cube_timings() {
        let dir = temp_dir("timings");
        let path = dir.0.join("big.jpg");
        let eq = panorama(8192);
        std::fs::write(&path, crate::export::encode_jpeg(&cubemap::render(&eq, 2048, &cubemap::STRIP), 90).unwrap())
            .unwrap();
        for max_dim in [Some(processor::PREVIEW_DIM), None] {
            let t = std::time::Instant::now();
            let l = load(&path, max_dim).unwrap();
            println!("{max_dim:?}: {:?} → {:?}", t.elapsed(), l.image.dimensions());
        }
    }
}

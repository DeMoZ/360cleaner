//! Кубическая карта (cubemap) из панорамы equirectangular: шесть квадратных граней по 90°, какими их видит
//! зритель изнутри куба. Сторона грани — четверть ширины панорамы: на экваторе та же плотность пикселей.
//!
//! Оси: x — вправо, y — вверх, z — вперёд (середина панорамы). У боковых граней верх — небо; у верхней грани
//! нижний край прилегает к передней, у нижней — верхний (как у krpano, Pannellum и креста 4×3).
//!
//! Обратно (`to_equirect`) — панорама 4n×2n из шести граней: так открываются фото, сохранённые кубической картой.

use image::RgbImage;
use rayon::prelude::*;
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Front,
    Right,
    Back,
    Left,
    Up,
    Down,
}

impl Face {
    /// Порядок отдельных файлов.
    pub const ALL: [Face; 6] = [Face::Front, Face::Right, Face::Back, Face::Left, Face::Up, Face::Down];

    /// Окончание имени файла грани: `pano_f.jpg` … `pano_d.jpg` (как у krpano и Pannellum).
    pub fn suffix(self) -> &'static str {
        match self {
            Face::Front => "f",
            Face::Right => "r",
            Face::Back => "b",
            Face::Left => "l",
            Face::Up => "u",
            Face::Down => "d",
        }
    }

    /// Место в `ALL`.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Направление взгляда в точку грани: `u` — слева направо, `v` — сверху вниз, оба от −1 до 1.
    fn direction(self, u: f32, v: f32) -> [f32; 3] {
        match self {
            Face::Front => [u, -v, 1.0],
            Face::Right => [1.0, -v, -u],
            Face::Back => [-u, -v, -1.0],
            Face::Left => [-1.0, -v, u],
            Face::Up => [u, 1.0, v],
            Face::Down => [u, -1.0, -v],
        }
    }
}

/// Обратное к `Face::direction`: грань, куда смотрит `d`, и точка на ней (`u`, `v` от −1 до 1).
fn face_uv([x, y, z]: [f32; 3]) -> (Face, f32, f32) {
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        if x > 0.0 { (Face::Right, -z / ax, -y / ax) } else { (Face::Left, z / ax, -y / ax) }
    } else if ay >= az {
        if y > 0.0 { (Face::Up, x / ay, z / ay) } else { (Face::Down, x / ay, -z / ay) }
    } else if z > 0.0 {
        (Face::Front, x / az, -y / az)
    } else {
        (Face::Back, -x / az, -y / az)
    }
}

/// Грань и её клетка (столбец, строка) в одном файле; клетка — квадрат со стороной грани.
pub type Cell = (Face, u32, u32);

/// Полоса 6×1 в порядке +X −X +Y −Y +Z −Z (Unity и другие движки).
pub const STRIP: [Cell; 6] = [
    (Face::Right, 0, 0),
    (Face::Left, 1, 0),
    (Face::Up, 2, 0),
    (Face::Down, 3, 0),
    (Face::Front, 4, 0),
    (Face::Back, 5, 0),
];

/// Крест 4×3: верх и низ над и под передней гранью, пустые клетки чёрные.
pub const CROSS: [Cell; 6] = [
    (Face::Up, 1, 0),
    (Face::Left, 0, 1),
    (Face::Front, 1, 1),
    (Face::Right, 2, 1),
    (Face::Back, 3, 1),
    (Face::Down, 1, 2),
];

/// Сторона грани для панорамы шириной `w`.
pub fn face_size(w: u32) -> u32 {
    ((w + 2) / 4).max(1)
}

/// Веса Catmull-Rom для сдвига `t` от 0 до 1.
fn cubic_weights(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [0.5 * (-t3 + 2.0 * t2 - t), 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0), 0.5 * (-3.0 * t3 + 4.0 * t2 + t), 0.5 * (t3 - t2)]
}

/// Цвет панорамы в направлении `d`: бикубическая выборка, по горизонтали панорама замкнута.
pub fn sample(eq: &RgbImage, d: [f32; 3]) -> [u8; 3] {
    let (w, h) = eq.dimensions();
    let [x, y, z] = d;
    let lon = x.atan2(z);
    let lat = y.atan2(x.hypot(z));
    let fx = (lon / TAU + 0.5) * w as f32 - 0.5;
    let fy = (0.5 - lat / PI) * h as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (wx, wy) = (cubic_weights(fx - x0), cubic_weights(fy - y0));
    let (x0, y0) = (x0 as i64, y0 as i64);
    let raw = eq.as_raw();
    let mut acc = [0f32; 3];
    for (j, &ky) in wy.iter().enumerate() {
        let row = (y0 + j as i64 - 1).clamp(0, h as i64 - 1) as usize * w as usize;
        for (i, &kx) in wx.iter().enumerate() {
            let col = (x0 + i as i64 - 1).rem_euclid(w as i64) as usize;
            let p = &raw[(row + col) * 3..(row + col) * 3 + 3];
            let k = kx * ky;
            for c in 0..3 {
                acc[c] += p[c] as f32 * k;
            }
        }
    }
    acc.map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// Бикубическая выборка внутри грани в точке (`fx`, `fy`) в пикселях; за краем — крайние пиксели.
fn sample_face(face: &RgbImage, fx: f32, fy: f32) -> [u8; 3] {
    let n = face.width() as i64;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (wx, wy) = (cubic_weights(fx - x0), cubic_weights(fy - y0));
    let (x0, y0) = (x0 as i64, y0 as i64);
    let raw = face.as_raw();
    let mut acc = [0f32; 3];
    for (j, &ky) in wy.iter().enumerate() {
        let row = (y0 + j as i64 - 1).clamp(0, n - 1) as usize * n as usize;
        for (i, &kx) in wx.iter().enumerate() {
            let col = (x0 + i as i64 - 1).clamp(0, n - 1) as usize;
            let p = &raw[(row + col) * 3..(row + col) * 3 + 3];
            let k = kx * ky;
            for c in 0..3 {
                acc[c] += p[c] as f32 * k;
            }
        }
    }
    acc.map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// Панорама 4n×2n из шести квадратных граней `faces` (порядок `Face::ALL`) со стороной n: та же плотность
/// пикселей на экваторе, что у граней.
pub fn to_equirect(faces: &[RgbImage; 6]) -> RgbImage {
    let n = faces[0].width();
    let (w, h) = (4 * n, 2 * n);
    let mut out = RgbImage::new(w, h);
    let half = n as f32 / 2.0;
    out.par_chunks_mut(w as usize * 3).enumerate().for_each(|(y, row)| {
        let lat = (0.5 - (y as f32 + 0.5) / h as f32) * PI;
        let (sl, cl) = lat.sin_cos();
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let lon = ((x as f32 + 0.5) / w as f32 - 0.5) * TAU;
            let (so, co) = lon.sin_cos();
            let (face, u, v) = face_uv([cl * so, sl, cl * co]);
            px.copy_from_slice(&sample_face(&faces[face.index()], (u + 1.0) * half - 0.5, (v + 1.0) * half - 0.5));
        }
    });
    out
}

/// Шесть граней (порядок `Face::ALL`) из картинки, где они разложены клетками `cells` со стороной `n`.
pub fn split_layout(img: &RgbImage, n: u32, cells: &[Cell]) -> [RgbImage; 6] {
    Face::ALL.map(|face| {
        let &(_, col, line) = cells.iter().find(|c| c.0 == face).expect("all six faces in layout");
        image::imageops::crop_imm(img, col * n, line * n, n, n).to_image()
    })
}

/// Кусок `tw`×`th` от точки (`x0`, `y0`) картинки из граней `cells` со стороной `n`; вне граней — чёрный.
pub fn render_region(eq: &RgbImage, n: u32, cells: &[Cell], x0: u32, y0: u32, tw: u32, th: u32) -> RgbImage {
    let mut out = RgbImage::new(tw, th);
    let inv = 2.0 / n as f32;
    out.par_chunks_mut(tw as usize * 3).enumerate().for_each(|(dy, row)| {
        let y = y0 + dy as u32;
        let v = ((y % n) as f32 + 0.5) * inv - 1.0;
        for (dx, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let x = x0 + dx as u32;
            let (col, line) = (x / n, y / n);
            if let Some(&(face, _, _)) = cells.iter().find(|c| c.1 == col && c.2 == line) {
                let u = ((x % n) as f32 + 0.5) * inv - 1.0;
                px.copy_from_slice(&sample(eq, face.direction(u, v)));
            }
        }
    });
    out
}

/// Размер картинки с гранями `cells` со стороной `n`.
pub fn layout_dims(n: u32, cells: &[Cell]) -> (u32, u32) {
    let cols = cells.iter().map(|c| c.1 + 1).max().unwrap_or(1);
    let rows = cells.iter().map(|c| c.2 + 1).max().unwrap_or(1);
    (cols * n, rows * n)
}

/// Картинка с гранями `cells` целиком.
pub fn render(eq: &RgbImage, n: u32, cells: &[Cell]) -> RgbImage {
    let (w, h) = layout_dims(n, cells);
    render_region(eq, n, cells, 0, 0, w, h)
}

/// Одна грань.
pub fn render_face(eq: &RgbImage, n: u32, face: Face) -> RgbImage {
    render(eq, n, &[(face, 0, 0)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Панорама, где цвет — функция направления: красный растёт вправо по долготе, синий — к небу.
    fn panorama(w: u32) -> RgbImage {
        RgbImage::from_fn(w, w / 2, |x, y| image::Rgb([(x * 256 / w) as u8, 128, (255 - y * 256 / (w / 2)) as u8]))
    }

    #[test]
    fn faces_look_where_expected() {
        let eq = panorama(1024);
        let n = face_size(1024);
        assert_eq!(n, 256);
        let mid = |f: Face| *render_face(&eq, n, f).get_pixel(n / 2, n / 2);
        // середины граней: вперёд — середина панорамы, вправо — три четверти, назад — край, влево — четверть
        let red = |f: Face| mid(f)[0] as i32;
        assert!((red(Face::Front) - 128).abs() <= 2);
        assert!((red(Face::Right) - 192).abs() <= 2);
        assert!((red(Face::Left) - 64).abs() <= 2);
        // у задней грани посередине шов панорамы, поэтому точка левее: там долгота atan2(0.5, −1)
        let back = render_face(&eq, n, Face::Back).get_pixel(n / 4, n / 2)[0] as f32;
        assert!((back - (0.5 + 0.5f32.atan2(-1.0) / TAU) * 256.0).abs() <= 2.0, "back {back}");
        // небо сверху, земля снизу
        assert!(mid(Face::Up)[2] > 250 && mid(Face::Down)[2] < 5);
        assert!((mid(Face::Front)[2] as i32 - 128).abs() <= 2);
    }

    #[test]
    fn neighbouring_faces_meet() {
        let eq = panorama(2048);
        let n = face_size(2048);
        let f = render_face(&eq, n, Face::Front);
        let near = |a: image::Rgb<u8>, b: image::Rgb<u8>| (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= 3);
        // правый край передней — левый край правой
        let r = render_face(&eq, n, Face::Right);
        assert!(near(*f.get_pixel(n - 1, n / 3), *r.get_pixel(0, n / 3)));
        // верхний край передней — нижний край верхней, нижний — верхний край нижней
        let u = render_face(&eq, n, Face::Up);
        assert!(near(*f.get_pixel(n / 3, 0), *u.get_pixel(n / 3, n - 1)));
        let d = render_face(&eq, n, Face::Down);
        assert!(near(*f.get_pixel(n / 3, n - 1), *d.get_pixel(n / 3, 0)));
        // левый край верхней — верхний край левой
        let l = render_face(&eq, n, Face::Left);
        assert!(near(*u.get_pixel(0, n / 3), *l.get_pixel(n / 3, 0)));
    }

    #[test]
    fn faces_back_to_panorama() {
        let eq = panorama(1024);
        let n = face_size(1024);
        for cells in [&STRIP[..], &CROSS[..]] {
            let layout = render(&eq, n, cells);
            let back = to_equirect(&split_layout(&layout, n, cells));
            assert_eq!(back.dimensions(), eq.dimensions());
            // кроме шва панорамы (там красный скачет от 255 к 0) и полюсов — те же цвета
            let mut worst = 0;
            for y in (40..472).step_by(7) {
                for x in (8..1016).step_by(5) {
                    let (a, b) = (eq.get_pixel(x, y), back.get_pixel(x, y));
                    worst = worst.max((0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap());
                }
            }
            assert!(worst <= 3, "worst {worst}");
        }
    }

    #[test]
    fn face_uv_inverts_direction() {
        for face in Face::ALL {
            for (u, v) in [(0.0, 0.0), (0.5, -0.3), (-0.9, 0.7)] {
                let (f, u2, v2) = face_uv(face.direction(u, v));
                assert_eq!(f, face);
                assert!((u - u2).abs() < 1e-5 && (v - v2).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn layouts_place_faces_in_cells() {
        let eq = panorama(512);
        let n = face_size(512);
        let strip = render(&eq, n, &STRIP);
        assert_eq!(strip.dimensions(), (6 * n, n));
        let front = render_face(&eq, n, Face::Front);
        assert_eq!(strip.get_pixel(4 * n + 10, 20), front.get_pixel(10, 20));
        let cross = render(&eq, n, &CROSS);
        assert_eq!(cross.dimensions(), (4 * n, 3 * n));
        assert_eq!(cross.get_pixel(n + 10, n + 20), front.get_pixel(10, 20));
        assert_eq!(cross.get_pixel(5, 5), &image::Rgb([0, 0, 0]));
        // кусок совпадает с тем же местом целой картинки
        let part = render_region(&eq, n, &STRIP, n - 7, 3, 30, 11);
        assert_eq!(part.get_pixel(12, 4), strip.get_pixel(n - 7 + 12, 7));
    }
}

//! Core image processing engine for 360-degree panorama person removal.
//! Pure classical Computer Vision in Rust with parallel Rayon execution.

use image::{ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;
use std::f32::consts::PI;
use std::path::Path;

/// Perceptual Lab color
#[derive(Clone, Copy)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

#[inline(always)]
fn rgb_to_lab(r: u8, g: u8, b: u8) -> Lab {
    // sRGB to linear RGB
    let to_linear = |c: u8| -> f32 {
        let v = (c as f32) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let lr = to_linear(r);
    let lg = to_linear(g);
    let lb = to_linear(b);

    // Observer: 2°, Illuminant: D65
    let x = (lr * 0.4124564 + lg * 0.3575761 + lb * 0.1804375) / 0.95047;
    let y = (lr * 0.2126729 + lg * 0.7151522 + lb * 0.0721750) / 1.00000;
    let z = (lr * 0.0193339 + lg * 0.1191920 + lb * 0.9503041) / 1.08883;

    let f = |t: f32| -> f32 {
        if t > 0.008856 {
            t.cbrt()
        } else {
            (7.787 * t) + (16.0 / 116.0)
        }
    };

    let fx = f(x);
    let fy = f(y);
    let fz = f(z);

    Lab {
        l: (116.0 * fy) - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

#[inline(always)]
fn delta_e(c1: Lab, c2: Lab) -> f32 {
    let dl = c1.l - c2.l;
    let da = c1.a - c2.a;
    let db = c1.b - c2.b;
    (dl * dl + da * da + db * db).sqrt()
}

/// Circular distance between two angles in radians [0, 2*PI)
#[inline(always)]
pub fn circular_distance(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs() % (2.0 * PI);
    if diff > PI {
        2.0 * PI - diff
    } else {
        diff
    }
}

/// Load image from file and optionally downscale to max_dim for fast preview
pub fn load_image(path: &Path, max_dim: Option<u32>) -> Result<RgbImage, Box<dyn std::error::Error>> {
    let img = image::open(path)?.to_rgb8();
    if let Some(dim) = max_dim {
        let (w, h) = img.dimensions();
        if w > dim || h > dim {
            let scale = (dim as f32) / (w.max(h) as f32);
            let nw = (w as f32 * scale).round() as u32;
            let nh = (h as f32 * scale).round() as u32;
            let resized = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
            return Ok(resized);
        }
    }
    Ok(img)
}

/// Compute raw Lab difference map between two images
pub fn compute_difference_map(img1: &RgbImage, img2: &RgbImage) -> Vec<f32> {
    let (w, h) = img1.dimensions();
    let total = (w * h) as usize;
    let mut diff = vec![0.0f32; total];

    diff.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w {
                let p1 = img1.get_pixel(x, y as u32);
                let p2 = img2.get_pixel(x, y as u32);
                let lab1 = rgb_to_lab(p1[0], p1[1], p1[2]);
                let lab2 = rgb_to_lab(p2[0], p2[1], p2[2]);
                row[x as usize] = delta_e(lab1, lab2);
            }
        });

    diff
}

/// Apply morphological dilation with horizontal 360-degree circular wrapping
pub fn dilate_mask_360(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<bool> {
    if radius <= 0 {
        return mask.to_vec();
    }
    let mut result = vec![false; (w * h) as usize];
    let r2 = radius * radius;

    result
        .par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y_idx, row_out)| {
            let y = y_idx as i32;
            for x in 0..(w as i32) {
                let mut found = false;
                'outer: for dy in -radius..=radius {
                    let ny = y + dy;
                    if ny < 0 || ny >= (h as i32) {
                        continue;
                    }
                    let dy2 = dy * dy;
                    for dx in -radius..=radius {
                        if dx * dx + dy2 <= r2 {
                            // Circular wrap on horizontal X coordinate!
                            let nx = (x + dx).rem_euclid(w as i32) as u32;
                            let idx = (ny as u32 * w + nx) as usize;
                            if mask[idx] {
                                found = true;
                                break 'outer;
                            }
                        }
                    }
                }
                row_out[x as usize] = found;
            }
        });

    result
}

/// Angular cluster centers in 360 space
#[derive(Debug, Clone, Copy)]
pub struct ClusterAngles {
    pub angle_a: f32, // [0, 2*PI)
    pub angle_b: f32, // [0, 2*PI)
}

/// Clusters the difference mask into two canonical angular centers on S^1.
/// Guarantees canonical order: angle_a < angle_b.
pub fn find_cluster_angles(mask: &[bool], w: u32, h: u32) -> ClusterAngles {
    let mut points_cos_sin = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if mask[idx] {
                let theta = 2.0 * PI * (x as f32) / (w as f32);
                points_cos_sin.push((theta.cos(), theta.sin()));
            }
        }
    }

    if points_cos_sin.len() < 20 {
        return ClusterAngles {
            angle_a: 0.0,
            angle_b: PI,
        };
    }

    // Deterministic antipodal initialization:
    // Center 0 is initialized from the mean direction of all points
    let (mut sum_cos, mut sum_sin) = (0.0f32, 0.0f32);
    for &(c, s) in &points_cos_sin {
        sum_cos += c;
        sum_sin += s;
    }
    let norm = (sum_cos * sum_cos + sum_sin * sum_sin).sqrt().max(1e-5);
    let mut c0 = (sum_cos / norm, sum_sin / norm);
    // Center 1 is antipodal opposite
    let mut c1 = (-c0.0, -c0.1);

    // Run circular k-means
    for _ in 0..20 {
        let (mut sum_c0, mut sum_s0, mut count0) = (0.0f32, 0.0f32, 0usize);
        let (mut sum_c1, mut sum_s1, mut count1) = (0.0f32, 0.0f32, 0usize);

        for &(c, s) in &points_cos_sin {
            let dot0 = c * c0.0 + s * c0.1;
            let dot1 = c * c1.0 + s * c1.1;
            if dot0 >= dot1 {
                sum_c0 += c;
                sum_s0 += s;
                count0 += 1;
            } else {
                sum_c1 += c;
                sum_s1 += s;
                count1 += 1;
            }
        }

        if count0 > 0 {
            let n0 = (sum_c0 * sum_c0 + sum_s0 * sum_s0).sqrt().max(1e-5);
            c0 = (sum_c0 / n0, sum_s0 / n0);
        }
        if count1 > 0 {
            let n1 = (sum_c1 * sum_c1 + sum_s1 * sum_s1).sqrt().max(1e-5);
            c1 = (sum_c1 / n1, sum_s1 / n1);
        }
    }

    let mut ang0 = c0.1.atan2(c0.0).rem_euclid(2.0 * PI);
    let mut ang1 = c1.1.atan2(c1.0).rem_euclid(2.0 * PI);

    // Canonical sorting: always ang0 < ang1
    if ang0 > ang1 {
        std::mem::swap(&mut ang0, &mut ang1);
    }

    ClusterAngles {
        angle_a: ang0,
        angle_b: ang1,
    }
}

/// Blends img1 and img2 based on the difference mask and canonical cluster centers.
/// If swap is false: Cluster A is replaced with img2, Cluster B with img1.
/// If swap is true: Cluster B is replaced with img2, Cluster A with img1.
/// Uses soft horizontal circular feathering for invisible seams.
pub fn blend_panoramas(
    img1: &RgbImage,
    img2: &RgbImage,
    mask: &[bool],
    angles: ClusterAngles,
    swap: bool,
    feather_radius: i32,
) -> RgbImage {
    let (w, h) = img1.dimensions();
    let mut target_mask = vec![false; (w * h) as usize];

    // For every pixel in the mask, assign it to Cluster A or Cluster B
    // purely based on circular distance to angle_a vs angle_b.
    // This is 100% resolution invariant!
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if mask[idx] {
                let theta = 2.0 * PI * (x as f32) / (w as f32);
                let dist_a = circular_distance(theta, angles.angle_a);
                let dist_b = circular_distance(theta, angles.angle_b);
                let is_cluster_a = dist_a <= dist_b;

                // If not swap: Cluster A replaced by img2
                // If swap: Cluster B replaced by img2
                if (!swap && is_cluster_a) || (swap && !is_cluster_a) {
                    target_mask[idx] = true;
                }
            }
        }
    }

    // Compute smooth alpha map with circular horizontal wrapping
    let alpha_map = if feather_radius > 0 {
        compute_feathered_alpha(&target_mask, w, h, feather_radius)
    } else {
        target_mask.iter().map(|&b| if b { 1.0f32 } else { 0.0f32 }).collect()
    };

    let mut result = ImageBuffer::new(w, h);
    let p1_raw = img1.as_raw();
    let p2_raw = img2.as_raw();

    result
        .enumerate_rows_mut()
        .par_bridge()
        .for_each(|(y, row)| {
            for (x, _, pixel) in row {
                let idx = (y * w + x) as usize;
                let a = alpha_map[idx];
                let p1_idx = idx * 3;
                let r1 = p1_raw[p1_idx] as f32;
                let g1 = p1_raw[p1_idx + 1] as f32;
                let b1 = p1_raw[p1_idx + 2] as f32;

                let r2 = p2_raw[p1_idx] as f32;
                let g2 = p2_raw[p1_idx + 1] as f32;
                let b2 = p2_raw[p1_idx + 2] as f32;

                let r = (1.0 - a) * r1 + a * r2;
                let g = (1.0 - a) * g1 + a * g2;
                let b = (1.0 - a) * b1 + a * b2;

                *pixel = Rgb([r.round() as u8, g.round() as u8, b.round() as u8]);
            }
        });

    result
}

/// Horizontal circular box/gaussian blur approximation for smooth feathering
fn compute_feathered_alpha(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<f32> {
    let alpha: Vec<f32> = mask.iter().map(|&b| if b { 1.0f32 } else { 0.0f32 }).collect();
    if radius <= 0 {
        return alpha;
    }

    let r = radius.min(50);
    let diameter = (2 * r + 1) as f32;

    // Horizontal pass with circular wrap-around
    let mut temp = vec![0.0f32; (w * h) as usize];
    temp.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y_idx, row_out)| {
            let y = y_idx as u32;
            let row_in = &alpha[(y * w) as usize..((y + 1) * w) as usize];
            let mut sum = 0.0f32;
            for dx in -r..=r {
                let x = dx.rem_euclid(w as i32) as usize;
                sum += row_in[x];
            }
            row_out[0] = sum / diameter;

            for x in 1..w {
                let add_x = (x as i32 + r).rem_euclid(w as i32) as usize;
                let rem_x = (x as i32 - r - 1).rem_euclid(w as i32) as usize;
                sum += row_in[add_x] - row_in[rem_x];
                row_out[x as usize] = (sum / diameter).clamp(0.0, 1.0);
            }
        });

    // Vertical pass with clamp
    let mut result = vec![0.0f32; (w * h) as usize];
    result
        .par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y_idx, row_out)| {
            let y = y_idx as i32;
            for x in 0..w {
                let mut sum = 0.0f32;
                let mut count = 0;
                for dy in -r..=r {
                    let ny = y + dy;
                    if ny >= 0 && ny < (h as i32) {
                        sum += temp[(ny as u32 * w + x) as usize];
                        count += 1;
                    }
                }
                row_out[x as usize] = (sum / (count as f32)).clamp(0.0, 1.0);
            }
        });

    result
}

/// Create colored visualization of the detected masks (Zone A = Cyan, Zone B = Red)
pub fn create_mask_visualization(
    mask: &[bool],
    w: u32,
    h: u32,
    angles: ClusterAngles,
) -> RgbImage {
    let mut vis = ImageBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if mask[idx] {
                let theta = 2.0 * PI * (x as f32) / (w as f32);
                let dist_a = circular_distance(theta, angles.angle_a);
                let dist_b = circular_distance(theta, angles.angle_b);
                if dist_a <= dist_b {
                    vis.put_pixel(x, y, Rgb([56, 189, 248])); // Zone A: Cyan
                } else {
                    vis.put_pixel(x, y, Rgb([239, 68, 68]));  // Zone B: Red
                }
            } else {
                vis.put_pixel(x, y, Rgb([15, 23, 42])); // Dark Background
            }
        }
    }
    vis
}

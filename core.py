"""
Core processing engine for 360-degree panorama person removal.
Works purely with classical Computer Vision (no neural networks / AI).
"""

import os
import math
import struct
import numpy as np

try:
    import cv2
except ImportError:
    cv2 = None

from PIL import Image


def load_image_from_pil(pil_img, max_dim=None):
    """Convert PIL image to numpy RGB and optionally scale."""
    rgb = np.array(pil_img.convert("RGB"))
    if max_dim is not None:
        h, w = rgb.shape[:2]
        if max(h, w) > max_dim:
            scale = max_dim / float(max(h, w))
            new_w, new_h = int(round(w * scale)), int(round(h * scale))
            if cv2 is not None:
                rgb = cv2.resize(rgb, (new_w, new_h), interpolation=cv2.INTER_AREA)
            else:
                p = pil_img.resize((new_w, new_h), Image.Resampling.BILINEAR)
                rgb = np.array(p)
    return rgb


def load_image(path, max_dim=None):
    """
    Load an image from disk.
    If max_dim is provided, scale image so max(width, height) <= max_dim for fast preview.
    Returns RGB numpy array (uint8).
    """
    if cv2 is not None:
        bgr = cv2.imread(path, cv2.IMREAD_COLOR)
        if bgr is None:
            raise FileNotFoundError(f"Could not open image at {path}")
        rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
    else:
        with Image.open(path) as pil_img:
            rgb = np.array(pil_img.convert("RGB"))

    if max_dim is not None:
        h, w = rgb.shape[:2]
        if max(h, w) > max_dim:
            scale = max_dim / float(max(h, w))
            new_w, new_h = int(round(w * scale)), int(round(h * scale))
            if cv2 is not None:
                rgb = cv2.resize(rgb, (new_w, new_h), interpolation=cv2.INTER_AREA)
            else:
                pil_temp = Image.fromarray(rgb)
                pil_temp = pil_temp.resize((new_w, new_h), Image.Resampling.BILINEAR)
                rgb = np.array(pil_temp)
    return rgb


def pad_360_horizontal(img, pad_width):
    """
    Apply circular horizontal wrap padding for 360 equirectangular images.
    Pad left with right edge, and pad right with left edge.
    """
    if pad_width <= 0:
        return img
    pad_width = min(pad_width, img.shape[1])
    left_pad = img[:, -pad_width:]
    right_pad = img[:, :pad_width]
    return np.concatenate([left_pad, img, right_pad], axis=1)


def unpad_360_horizontal(img, pad_width):
    """Remove circular horizontal wrap padding."""
    if pad_width <= 0:
        return img
    return img[:, pad_width:-pad_width]


def compute_difference_mask(img1, img2, threshold=24, dilate_radius=25, min_area=400):
    """
    Compute difference mask between two registered 360 photos.
    Uses Lab color space distance to be sensitive to human clothes & skin against background.
    Handles 360 wrap-around boundary seamlessly.
    """
    h, w = img1.shape[:2]
    pad_x = max(32, int(w * 0.05))

    if cv2 is not None:
        # Convert RGB to Lab for perceptual distance
        lab1 = cv2.cvtColor(img1, cv2.COLOR_RGB2LAB).astype(np.float32)
        lab2 = cv2.cvtColor(img2, cv2.COLOR_RGB2LAB).astype(np.float32)
        # Delta E approximation (Euclidean in Lab)
        delta = np.linalg.norm(lab1 - lab2, axis=2)

        # Pad horizontally for circular wrapping
        delta_padded = pad_360_horizontal(delta, pad_x)

        # Gaussian blur to filter sensor grain
        blurred = cv2.GaussianBlur(delta_padded, (7, 7), 0)

        # Threshold to binary
        binary = (blurred > threshold).astype(np.uint8) * 255

        # Morphological opening: eliminates small leaf/wind noise
        open_k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (5, 5))
        cleaned = cv2.morphologyEx(binary, cv2.MORPH_OPEN, open_k)

        # Morphological closing & dilation: fills body silhouette and clothing gaps
        close_k_size = max(9, int(dilate_radius * 0.8) | 1)
        close_k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (close_k_size, close_k_size))
        closed = cv2.morphologyEx(cleaned, cv2.MORPH_CLOSE, close_k)

        dilate_k_size = max(5, int(dilate_radius) | 1)
        dilate_k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (dilate_k_size, dilate_k_size))
        dilated = cv2.dilate(closed, dilate_k, iterations=1)

        # Remove horizontal circular padding
        mask = unpad_360_horizontal(dilated, pad_x)

        # Filter out tiny connected components
        num_labels, labels, stats, _ = cv2.connectedComponentsWithStats(mask)
        filtered_mask = np.zeros_like(mask)
        for i in range(1, num_labels):
            if stats[i, cv2.CC_STAT_AREA] >= min_area:
                filtered_mask[labels == i] = 255
        return filtered_mask
    else:
        # Fallback without OpenCV
        diff = np.max(np.abs(img1.astype(np.int16) - img2.astype(np.int16)), axis=2)
        mask = (diff > threshold).astype(np.uint8) * 255
        return mask


def split_clusters_angular(mask, img_w):
    """
    Separates the changed regions into two angular clusters (Person in Photo 1 vs Person in Photo 2).
    Uses circular coordinate embedding (cos(theta), sin(theta)) to be 100% immune to 0/360 boundary cuts.
    Returns: (mask_A, mask_B, angle_A_deg, angle_B_deg)
    """
    ys, xs = np.where(mask > 0)
    if len(xs) < 50:
        # Mask is empty or negligible
        return mask, np.zeros_like(mask), 0.0, 180.0

    # Convert x coordinate to angular theta on [0, 2*pi)
    thetas = 2.0 * np.pi * xs / float(img_w)
    u = np.cos(thetas)
    v = np.sin(thetas)
    points = np.stack([u, v], axis=1)

    # Simple, deterministic circular 2-means clustering
    # Initialize with two opposite poles based on mean direction or extremes
    # Pick a random sample and its antipodal point
    np.random.seed(42)
    idx0 = 0
    center0 = points[idx0].copy()
    # Find point with minimum dot product (most opposite)
    dots = np.dot(points, center0)
    idx1 = np.argmin(dots)
    center1 = points[idx1].copy()

    # Normalize centers
    norm0 = np.linalg.norm(center0)
    norm1 = np.linalg.norm(center1)
    if norm0 > 1e-6: center0 /= norm0
    if norm1 > 1e-6: center1 /= norm1

    for _ in range(15):
        d0 = np.dot(points, center0)
        d1 = np.dot(points, center1)
        cluster0_mask = d0 >= d1
        cluster1_mask = ~cluster0_mask

        if np.any(cluster0_mask):
            new_c0 = np.mean(points[cluster0_mask], axis=0)
            n0 = np.linalg.norm(new_c0)
            if n0 > 1e-6: center0 = new_c0 / n0

        if np.any(cluster1_mask):
            new_c1 = np.mean(points[cluster1_mask], axis=0)
            n1 = np.linalg.norm(new_c1)
            if n1 > 1e-6: center1 = new_c1 / n1

    # Now classify every pixel of the entire width
    all_x = np.arange(img_w)
    all_thetas = 2.0 * np.pi * all_x / float(img_w)
    all_u = np.cos(all_thetas)
    all_v = np.sin(all_thetas)
    all_pts = np.stack([all_u, all_v], axis=1)

    score0 = np.dot(all_pts, center0)
    score1 = np.dot(all_pts, center1)
    is_cluster0_x = score0 >= score1  # shape (img_w,)

    mask_A = np.zeros_like(mask)
    mask_B = np.zeros_like(mask)

    # Broadcast x assignment
    mask_A = (mask > 0) & is_cluster0_x[np.newaxis, :]
    mask_B = (mask > 0) & (~is_cluster0_x[np.newaxis, :])

    mask_A = (mask_A.astype(np.uint8)) * 255
    mask_B = (mask_B.astype(np.uint8)) * 255

    angle_A = (math.atan2(center0[1], center0[0]) * 180.0 / math.pi) % 360.0
    angle_B = (math.atan2(center1[1], center1[0]) * 180.0 / math.pi) % 360.0

    return mask_A, mask_B, angle_A, angle_B


def blend_images(img1, img2, mask_A, mask_B, swap=False, feather_radius=20):
    """
    Blends img1 and img2 to produce the clean 360 photo.
    In zone where person 1 is standing: replace with img2.
    In zone where person 2 is standing: replace with img1.
    If swap is True, flips the assignment.
    Uses circular horizontal feathered boundary for seamless transitions.
    """
    h, w = img1.shape[:2]
    pad_x = max(32, int(w * 0.05))

    # Determine which mask gets replaced by which photo
    # Let target_mask be the areas of img1 that should be replaced with img2
    if not swap:
        # Cluster A is replaced by img2, Cluster B stays img1
        replace_with_img2 = mask_A
    else:
        # Cluster B is replaced by img2, Cluster A stays img1
        replace_with_img2 = mask_B

    if cv2 is not None:
        # Apply circular padding to mask before blurring to avoid boundary seam
        padded_mask = pad_360_horizontal(replace_with_img2.astype(np.float32) / 255.0, pad_x)

        # Smooth feather transition
        k_size = max(5, int(feather_radius * 2) | 1)
        sigma = feather_radius / 2.0
        smoothed_padded = cv2.GaussianBlur(padded_mask, (k_size, k_size), sigma)

        alpha = unpad_360_horizontal(smoothed_padded, pad_x)
        alpha = np.clip(alpha, 0.0, 1.0)[:, :, np.newaxis]

        # Multi-band / alpha blend
        result = (1.0 - alpha) * img1.astype(np.float32) + alpha * img2.astype(np.float32)
        return np.clip(result, 0, 255).astype(np.uint8)
    else:
        alpha = (replace_with_img2.astype(np.float32) / 255.0)[:, :, np.newaxis]
        result = (1.0 - alpha) * img1.astype(np.float32) + alpha * img2.astype(np.float32)
        return np.clip(result, 0, 255).astype(np.uint8)


def copy_xmp_and_exif(source_jpeg_path, target_jpeg_path):
    """
    Copies APP1 markers (EXIF & XMP 360 metadata) from source 360 JPEG to target JPEG.
    Ensures Google Photos, Insta360, Facebook, VR headsets recognize it as equirectangular 360!
    """
    try:
        with open(source_jpeg_path, 'rb') as f:
            src_bytes = f.read()
        with open(target_jpeg_path, 'rb') as f:
            dst_bytes = f.read()

        if not src_bytes.startswith(b'\xff\xd8') or not dst_bytes.startswith(b'\xff\xd8'):
            return False

        # Extract APP1 (0xFFE1) and APP2 (0xFFE2) markers from source
        markers = []
        idx = 2
        src_len = len(src_bytes)
        while idx < src_len - 4:
            if src_bytes[idx] != 0xff:
                break
            marker = src_bytes[idx:idx+2]
            if marker in (b'\xff\xd9', b'\xff\xda'):
                break
            seg_len = struct.unpack('>H', src_bytes[idx+2:idx+4])[0]
            seg_data = src_bytes[idx:idx+2+seg_len]
            if marker in (b'\xff\xe1', b'\xff\xe2'):
                markers.append(seg_data)
            idx += 2 + seg_len

        if not markers:
            return False

        # Construct new JPEG with injected markers right after SOI
        new_dst = b'\xff\xd8' + b''.join(markers) + dst_bytes[2:]
        with open(target_jpeg_path, 'wb') as f:
            f.write(new_dst)
        return True
    except Exception as e:
        print(f"Warning: could not copy 360 metadata: {e}")
        return False


def process_clean_plate(
    path1,
    path2,
    output_path=None,
    threshold=24,
    dilate_radius=25,
    feather_radius=20,
    swap=False,
    max_dim=None
):
    """
    Full pipeline to generate clean 360 plate.
    Returns: (result_rgb, mask_A, mask_B, angle_A, angle_B)
    """
    img1 = load_image(path1, max_dim=max_dim)
    img2 = load_image(path2, max_dim=max_dim)

    # Check dimension match
    if img1.shape != img2.shape:
        if cv2 is not None:
            img2 = cv2.resize(img2, (img1.shape[1], img1.shape[0]), interpolation=cv2.INTER_AREA)
        else:
            p2 = Image.fromarray(img2).resize((img1.shape[1], img1.shape[0]))
            img2 = np.array(p2)

    diff_mask = compute_difference_mask(img1, img2, threshold=threshold, dilate_radius=dilate_radius)
    mask_A, mask_B, angle_A, angle_B = split_clusters_angular(diff_mask, img1.shape[1])

    result = blend_images(img1, img2, mask_A, mask_B, swap=swap, feather_radius=feather_radius)

    if output_path is not None:
        # Save output using Pillow
        out_pil = Image.fromarray(result)
        out_pil.save(output_path, quality=97)
        copy_xmp_and_exif(path1, output_path)

    return result, mask_A, mask_B, angle_A, angle_B

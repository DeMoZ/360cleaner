//! XMP & EXIF 360-degree panorama metadata preservation for JPEG files.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

/// Extracts APP1 (Exif & XMP 360) and APP2 (ICC) segment markers from a JPEG.
pub fn extract_jpeg_markers(path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let mut markers = Vec::new();
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Ok(markers);
    }

    let mut idx = 2;
    while idx + 4 <= bytes.len() {
        if bytes[idx] != 0xFF {
            break;
        }
        let marker = bytes[idx + 1];
        if marker == 0xD9 || marker == 0xDA {
            // End of image or Start of scan
            break;
        }

        let seg_len = ((bytes[idx + 2] as usize) << 8) | (bytes[idx + 3] as usize);
        if idx + 2 + seg_len > bytes.len() {
            break;
        }

        // We care about APP1 (0xE1: Exif, XMP Photosphere) and APP2 (0xE2: ICC color profile)
        if marker == 0xE1 || marker == 0xE2 {
            markers.push(bytes[idx..idx + 2 + seg_len].to_vec());
        }

        idx += 2 + seg_len;
    }

    Ok(markers)
}

/// Injects metadata markers into target JPEG right after SOI (0xFFD8).
pub fn inject_markers_to_jpeg(
    target_path: &Path,
    markers: &[Vec<u8>],
) -> Result<(), Box<dyn std::error::Error>> {
    if markers.is_empty() {
        return Ok(());
    }

    let mut file = File::open(target_path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Ok(());
    }

    // Build new JPEG buffer
    let mut new_bytes = Vec::with_capacity(bytes.len() + 65536);
    new_bytes.extend_from_slice(&[0xFF, 0xD8]);

    for marker in markers {
        new_bytes.extend_from_slice(marker);
    }

    new_bytes.extend_from_slice(&bytes[2..]);

    let mut out_file = File::create(target_path)?;
    out_file.write_all(&new_bytes)?;

    Ok(())
}

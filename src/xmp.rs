//! Перенос метаданных 360° (XMP GPano, EXIF) и цветового профиля ICC из исходного JPEG в результат.
//! Без них Google Photos, Facebook, VR-шлемы и Insta360 Studio не узнают в файле сферическую панораму.

/// Сегменты APP1 (EXIF, XMP, расширенный XMP) и APP2 (ICC) из JPEG целиком, с маркером и длиной.
pub fn extract_jpeg_markers(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut markers = Vec::new();
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return markers;
    }
    let mut idx = 2;
    while idx + 4 <= bytes.len() {
        if bytes[idx] != 0xFF {
            break;
        }
        let marker = bytes[idx + 1];
        if marker == 0xFF {
            // заполнитель между сегментами
            idx += 1;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        let seg_len = ((bytes[idx + 2] as usize) << 8) | bytes[idx + 3] as usize;
        if seg_len < 2 || idx + 2 + seg_len > bytes.len() {
            break;
        }
        if marker == 0xE1 || marker == 0xE2 {
            markers.push(bytes[idx..idx + 2 + seg_len].to_vec());
        }
        idx += 2 + seg_len;
    }
    markers
}

/// Вставляет сегменты в JPEG: после SOI и сегмента JFIF (APP0), если он есть.
pub fn inject_markers(jpeg: &[u8], markers: &[Vec<u8>]) -> Vec<u8> {
    if markers.is_empty() || jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut at = 2;
    if jpeg.len() >= 6 && jpeg[2] == 0xFF && jpeg[3] == 0xE0 {
        let len = ((jpeg[4] as usize) << 8) | jpeg[5] as usize;
        if 2 + 2 + len <= jpeg.len() {
            at = 4 + len;
        }
    }
    let extra: usize = markers.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(jpeg.len() + extra);
    out.extend_from_slice(&jpeg[..at]);
    for m in markers {
        out.extend_from_slice(m);
    }
    out.extend_from_slice(&jpeg[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let len = payload.len() + 2;
        let mut v = vec![0xFF, marker, (len >> 8) as u8, len as u8];
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn copies_app1_app2_after_jfif() {
        let xmp = seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0<GPano/>");
        let icc = seg(0xE2, b"ICC_PROFILE\0..");
        let mut src = vec![0xFF, 0xD8];
        src.extend(seg(0xE0, b"JFIF\0"));
        src.extend(&xmp);
        src.extend(seg(0xEC, b"Ducky"));
        src.extend(&icc);
        src.extend([0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]);
        let markers = extract_jpeg_markers(&src);
        assert_eq!(markers, vec![xmp.clone(), icc.clone()]);

        let mut target = vec![0xFF, 0xD8];
        let jfif = seg(0xE0, b"JFIF\0\x01\x02");
        target.extend(&jfif);
        target.extend([0xFF, 0xDB, 0, 2, 0xFF, 0xD9]);
        let out = inject_markers(&target, &markers);
        let mut expected = vec![0xFF, 0xD8];
        expected.extend(&jfif);
        expected.extend(&xmp);
        expected.extend(&icc);
        expected.extend([0xFF, 0xDB, 0, 2, 0xFF, 0xD9]);
        assert_eq!(out, expected);
    }

    #[test]
    fn not_a_jpeg() {
        assert!(extract_jpeg_markers(b"\x89PNG....").is_empty());
        assert_eq!(inject_markers(b"abc", &[vec![1]]), b"abc");
    }
}

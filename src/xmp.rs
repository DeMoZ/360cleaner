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

const XMP_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Размеры GPano в пикселях и направление: true — по ширине.
const GPANO_PIXELS: [(&str, bool); 6] = [
    ("FullPanoWidthPixels", true),
    ("CroppedAreaImageWidthPixels", true),
    ("CroppedAreaLeftPixels", true),
    ("FullPanoHeightPixels", false),
    ("CroppedAreaImageHeightPixels", false),
    ("CroppedAreaTopPixels", false),
];

/// Пересчитывает размеры GPano в XMP после уменьшения кадра `from` → `to`: иначе просмотрщики 360° ждут исходный
/// размер и показывают панораму неправильно. Остальные сегменты не меняются.
pub fn scale_gpano(markers: Vec<Vec<u8>>, from: (u32, u32), to: (u32, u32)) -> Vec<Vec<u8>> {
    if from == to || from.0 == 0 || from.1 == 0 {
        return markers;
    }
    markers.into_iter().map(|seg| scale_segment(&seg, from, to).unwrap_or(seg)).collect()
}

fn scale_segment(seg: &[u8], from: (u32, u32), to: (u32, u32)) -> Option<Vec<u8>> {
    if seg.len() < 4 || seg[1] != 0xE1 || !seg[4..].starts_with(XMP_NS) {
        return None;
    }
    let body = std::str::from_utf8(&seg[4 + XMP_NS.len()..]).ok()?;
    let mut text = body.to_string();
    for (key, horizontal) in GPANO_PIXELS {
        let (num, den) = if horizontal { (to.0, from.0) } else { (to.1, from.1) };
        let scale = |v: u64| (v * num as u64 + den as u64 / 2) / den as u64;
        text = replace_numbers(&text, &format!("GPano:{key}"), scale);
    }
    if text == body {
        return None;
    }
    let len = 2 + XMP_NS.len() + text.len();
    if len > 0xFFFF {
        return None;
    }
    let mut out = vec![0xFF, 0xE1, (len >> 8) as u8, len as u8];
    out.extend_from_slice(XMP_NS);
    out.extend_from_slice(text.as_bytes());
    Some(out)
}

/// Сегменты без XMP (и его продолжений): для граней кубической карты, которые уже не сферическая панорама —
/// иначе просмотрщики по GPano приняли бы квадрат за 360°. EXIF и ICC остаются.
pub fn without_xmp(markers: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    markers
        .into_iter()
        .filter(|seg| {
            !(seg.len() > 4 && seg[1] == 0xE1 && (seg[4..].starts_with(XMP_NS) || seg[4..].starts_with(XMP_EXT_NS)))
        })
        .collect()
}

/// Сегмент XMP с GPano для панорамы `w`×`h` без обрезки: панораме из кубической карты своего GPano неоткуда
/// взять, а без него просмотрщики 360° не узнают в файле сферу.
pub fn gpano(w: u32, h: u32) -> Vec<u8> {
    let xml = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:GPano=\"http://ns.google.com/photos/1.0/panorama/\" \
         GPano:ProjectionType=\"equirectangular\" GPano:UsePanoramaViewer=\"True\" \
         GPano:FullPanoWidthPixels=\"{w}\" GPano:FullPanoHeightPixels=\"{h}\" \
         GPano:CroppedAreaImageWidthPixels=\"{w}\" GPano:CroppedAreaImageHeightPixels=\"{h}\" \
         GPano:CroppedAreaLeftPixels=\"0\" GPano:CroppedAreaTopPixels=\"0\"/>\
         </rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    );
    let len = 2 + XMP_NS.len() + xml.len();
    let mut seg = vec![0xFF, 0xE1, (len >> 8) as u8, len as u8];
    seg.extend_from_slice(XMP_NS);
    seg.extend_from_slice(xml.as_bytes());
    seg
}

const XMP_EXT_NS: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// Заменяет число после `key` в обеих записях XMP: атрибутом `key="123"` и элементом `<key>123</key>`.
fn replace_numbers(text: &str, key: &str, f: impl Fn(u64) -> u64) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(key) {
        let after = &rest[i + key.len()..];
        out.push_str(&rest[..i + key.len()]);
        rest = after;
        let open = if after.starts_with("=\"") || after.starts_with("='") {
            2
        } else if after.starts_with('>') {
            1
        } else {
            continue;
        };
        let digits = after[open..].bytes().take_while(u8::is_ascii_digit).count();
        let Ok(value) = after[open..open + digits].parse::<u64>() else { continue };
        out.push_str(&after[..open]);
        out.push_str(&f(value).to_string());
        rest = &after[open + digits..];
    }
    out.push_str(rest);
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
    fn scales_gpano_sizes() {
        let xmp = |w: u32, h: u32| {
            let body = format!(
                "<rdf:Description GPano:ProjectionType=\"equirectangular\" GPano:FullPanoWidthPixels=\"{w}\" \
                 GPano:CroppedAreaTopPixels='0'><GPano:FullPanoHeightPixels>{h}</GPano:FullPanoHeightPixels>\
                 <GPano:CroppedAreaImageWidthPixels>{w}</GPano:CroppedAreaImageWidthPixels></rdf:Description>"
            );
            let mut payload = XMP_NS.to_vec();
            payload.extend(body.as_bytes());
            seg(0xE1, &payload)
        };
        let exif = seg(0xE1, b"Exif\0\0MM 15520");
        let out = scale_gpano(vec![xmp(15520, 7760), exif.clone()], (15520, 7760), (8192, 4096));
        assert_eq!(out, vec![xmp(8192, 4096), exif.clone()]);
        assert_eq!(scale_gpano(vec![exif.clone()], (100, 50), (100, 50)), vec![exif]);
    }

    #[test]
    fn drops_xmp_keeps_exif_and_icc() {
        let exif = seg(0xE1, b"Exif\0\0MM");
        let icc = seg(0xE2, b"ICC_PROFILE\0..");
        let xmp = seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0<GPano/>");
        let ext = seg(0xE1, b"http://ns.adobe.com/xmp/extension/\0ABCD");
        assert_eq!(without_xmp(vec![exif.clone(), xmp, icc.clone(), ext]), vec![exif, icc]);
    }

    #[test]
    fn new_gpano_scales_like_camera_one() {
        let seg = gpano(4000, 2000);
        assert_eq!(((seg[2] as usize) << 8 | seg[3] as usize) + 2, seg.len());
        let scaled = scale_gpano(vec![seg], (4000, 2000), (2000, 1000));
        let text = String::from_utf8_lossy(&scaled[0]);
        assert!(
            text.contains("GPano:FullPanoWidthPixels=\"2000\"") && text.contains("ProjectionType=\"equirectangular\"")
        );
        assert!(without_xmp(scaled).is_empty());
    }

    #[test]
    fn not_a_jpeg() {
        assert!(extract_jpeg_markers(b"\x89PNG....").is_empty());
        assert_eq!(inject_markers(b"abc", &[vec![1]]), b"abc");
    }
}

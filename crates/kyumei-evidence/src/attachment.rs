//! Attachment metadata. Kyumei does not read screenshots; it records what
//! they are so the operator can describe what they show.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageInfo {
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn image_info(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some(ImageInfo {
            format: "PNG",
            width: w,
            height: h,
        });
    }
    if bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Some(ImageInfo {
            format: "GIF",
            width: w,
            height: h,
        });
    }
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if matches!(marker, 0xC0..=0xC3) {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some(ImageInfo {
                    format: "JPEG",
                    width: w,
                    height: h,
                });
            }
            i += 2 + len;
        }
    }
    None
}

pub(crate) fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    image_info(bytes).map(|i| match i.format {
        "PNG" => "image/png",
        "GIF" => "image/gif",
        _ => "image/jpeg",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_png_dimensions() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&720u32.to_be_bytes());
        let info = image_info(&png).expect("png");
        assert_eq!((info.width, info.height), (1280, 720));
    }
}

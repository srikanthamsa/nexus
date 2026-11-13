use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use jpeg_encoder::{ColorType, Encoder as JpegEncoder};

pub struct Preview {
    pub data_uri: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

pub fn encode_preview(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    quality: u8,
) -> Result<Preview, String> {
    let (preview, width, height) = downscale_bgra(bgra, width, height, max_width)
        .ok_or_else(|| "The captured frame was smaller than its declared size.".to_string())?;
    let mut jpeg = Vec::new();
    JpegEncoder::new(&mut jpeg, quality)
        .encode(&preview, width as u16, height as u16, ColorType::Bgra)
        .map_err(|error| format!("Could not encode the preview frame: {error}"))?;
    Ok(Preview {
        data_uri: format!("data:image/jpeg;base64,{}", BASE64.encode(&jpeg)),
        width,
        height,
        bytes: jpeg.len(),
    })
}

/// Cheap integer-sample downscale for the WebView preview. Capture and decode
/// frames arrive tightly packed as BGRA.
pub fn downscale_bgra(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected = width as usize * height as usize * 4;
    if bgra.len() < expected {
        return None;
    }
    if width <= max_width {
        return Some((bgra[..expected].to_vec(), width, height));
    }

    let factor = width.div_ceil(max_width);
    let out_width = (width / factor).max(1);
    let out_height = (height / factor).max(1);
    let mut out = vec![0u8; out_width as usize * out_height as usize * 4];
    let offset = (factor / 2) as usize;

    for y in 0..out_height as usize {
        let source_y = (y * factor as usize + offset).min(height as usize - 1);
        let source_row = source_y * width as usize;
        let target_row = y * out_width as usize;
        for x in 0..out_width as usize {
            let source_x = (x * factor as usize + offset).min(width as usize - 1);
            let source = (source_row + source_x) * 4;
            let target = (target_row + x) * 4;
            out[target..target + 4].copy_from_slice(&bgra[source..source + 4]);
        }
    }

    Some((out, out_width, out_height))
}

#[cfg(test)]
mod tests {
    use super::downscale_bgra;

    #[test]
    fn downscales_to_the_requested_width() {
        let width = 400u32;
        let height = 200u32;
        let frame = vec![7u8; width as usize * height as usize * 4];
        let (out, out_width, out_height) = downscale_bgra(&frame, width, height, 100).unwrap();
        assert_eq!(out_width, 100);
        assert_eq!(out_height, 50);
        assert_eq!(out.len(), 100 * 50 * 4);
    }

    #[test]
    fn keeps_frames_already_within_budget() {
        let frame = vec![9u8; 8 * 4 * 4];
        let (out, out_width, out_height) = downscale_bgra(&frame, 8, 4, 64).unwrap();
        assert_eq!((out_width, out_height), (8, 4));
        assert_eq!(out, frame);
    }

    #[test]
    fn rejects_truncated_frames() {
        assert!(downscale_bgra(&[0u8; 10], 8, 4, 64).is_none());
        assert!(downscale_bgra(&[], 0, 0, 64).is_none());
    }
}

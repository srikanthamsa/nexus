use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureOptions {
    pub display_index: Option<usize>,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub bitrate_mbps: u32,
    pub preview_fps: f64,
    pub preview_max_width: u32,
    pub shows_cursor: bool,
    /// Share the plain-text clipboard with the peer while a session is live.
    pub clipboard: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            display_index: None,
            width: 1920,
            height: 1080,
            frame_rate: 30.0,
            bitrate_mbps: 35,
            preview_fps: 12.0,
            preview_max_width: 1280,
            shows_cursor: true,
            clipboard: true,
        }
    }
}

impl CaptureOptions {
    pub fn normalized(mut self) -> Result<Self, String> {
        if !self.frame_rate.is_finite() || self.frame_rate < 1.0 || self.frame_rate > 240.0 {
            return Err("Frame rate must be between 1 and 240.".to_string());
        }
        if !self.preview_fps.is_finite() || self.preview_fps < 1.0 || self.preview_fps > 30.0 {
            return Err("Preview frame rate must be between 1 and 30.".to_string());
        }
        // HEVC requires even dimensions.
        self.width = (self.width.clamp(320, 7680) / 2) * 2;
        self.height = (self.height.clamp(240, 4320) / 2) * 2;
        self.bitrate_mbps = self.bitrate_mbps.clamp(2, 200);
        self.preview_max_width = self.preview_max_width.clamp(320, 3840);
        Ok(self)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
pub use platform::{CapturePipeline, CaptureTick};

#[cfg(target_os = "macos")]
mod platform {
    use super::{now_ms, CaptureOptions};
    use macos_media_toolkit::{
        screen::{ScreenCapture, ScreenCaptureConfig},
        videotoolbox::{EncoderConfig, HevcEncoder},
        BgraFrame,
    };
    use std::time::{Duration, Instant};

    pub struct EncodedFrame {
        pub annex_b: Vec<u8>,
        pub is_keyframe: bool,
        pub sequence: u64,
        pub timestamp_ms: u64,
        pub width: u32,
        pub height: u32,
        pub encode_ms: f64,
    }

    pub struct CapturedFrame {
        pub raw: BgraFrame,
        pub encoded: EncodedFrame,
    }

    pub enum CaptureTick {
        Frame(CapturedFrame),
        Idle,
        Stopped,
    }

    pub struct CapturePipeline {
        capture: ScreenCapture,
        encoder: HevcEncoder,
        options: CaptureOptions,
        sequence: u64,
        force_keyframe: bool,
    }

    impl CapturePipeline {
        pub fn open(options: &CaptureOptions) -> Result<Self, String> {
            let capture = ScreenCapture::open(&ScreenCaptureConfig {
                display_index: options.display_index,
                width: options.width,
                height: options.height,
                frame_rate: options.frame_rate,
                shows_cursor: options.shows_cursor,
                scales_to_fit: true,
            })
            .map_err(|error| format!("Could not start screen capture: {error}"))?;

            let encoder = HevcEncoder::new(&EncoderConfig {
                width: options.width,
                height: options.height,
                frame_rate: options.frame_rate,
                average_bitrate_bps: options.bitrate_mbps.saturating_mul(1_000_000),
                keyframe_interval: (options.frame_rate * 2.0).round().max(1.0) as usize,
            })
            .map_err(|error| format!("Could not start the HEVC encoder: {error}"))?;

            Ok(Self {
                capture,
                encoder,
                options: options.clone(),
                sequence: 0,
                force_keyframe: false,
            })
        }

        pub fn display(&self) -> &str {
            self.capture.display_description()
        }

        /// Makes the next encoded frame an IDR so a newly joined or desynced
        /// client can rebuild its decoder without waiting for the periodic keyframe.
        pub fn request_keyframe(&mut self) {
            self.force_keyframe = true;
        }

        /// Retargets the running hardware encoder without restarting capture.
        pub fn set_bitrate_mbps(&mut self, mbps: f64) -> Result<(), String> {
            let bitrate_bps = (mbps.clamp(2.0, 200.0) * 1_000_000.0).round() as u32;
            self.encoder
                .set_bitrate_and_frame_rate(bitrate_bps, self.options.frame_rate)
                .map_err(|error| format!("Could not retarget the encoder bitrate: {error}"))
        }

        pub fn next(&mut self, timeout: Duration) -> Result<CaptureTick, String> {
            let Some(raw) = self.capture.take_frame(timeout) else {
                if self.capture.is_stopped() {
                    return Ok(CaptureTick::Stopped);
                }
                return Ok(CaptureTick::Idle);
            };

            let encode_started = Instant::now();
            let keyframe_requested = self.sequence == 0 || self.force_keyframe;
            let encoded = self
                .encoder
                .encode(&raw.bgra, raw.width, raw.height, keyframe_requested)
                .map_err(|error| format!("Hardware encoding failed: {error}"))?;
            if keyframe_requested {
                self.force_keyframe = false;
            }
            let encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;
            let sequence = self.sequence;
            self.sequence += 1;

            Ok(CaptureTick::Frame(CapturedFrame {
                raw,
                encoded: EncodedFrame {
                    annex_b: encoded.annex_b,
                    is_keyframe: encoded.is_keyframe,
                    sequence,
                    timestamp_ms: now_ms(),
                    width: self.options.width,
                    height: self.options.height,
                    encode_ms,
                },
            }))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use macos_media_toolkit::videotoolbox::{DecoderConfig, HevcDecoder};

        #[test]
        fn round_trips_a_hardware_hevc_frame() {
            let width = 320u32;
            let height = 240u32;
            let mut pixels = Vec::with_capacity((width * height * 4) as usize);
            for y in 0..height {
                for x in 0..width {
                    pixels.extend_from_slice(&[
                        (x * 255 / width) as u8,
                        (y * 255 / height) as u8,
                        128,
                        255,
                    ]);
                }
            }

            let mut encoder = HevcEncoder::new(&EncoderConfig {
                width,
                height,
                frame_rate: 30.0,
                average_bitrate_bps: 4_000_000,
                keyframe_interval: 60,
            })
            .expect("HEVC encoder");
            let encoded = encoder
                .encode(&pixels, width, height, true)
                .expect("encode a keyframe");
            assert!(encoded.is_keyframe);
            assert!(!encoded.annex_b.is_empty());

            let mut decoder = HevcDecoder::new(&DecoderConfig::default());
            let decoded = decoder
                .decode(&encoded.annex_b)
                .expect("decode the keyframe");
            assert_eq!((decoded.frame.width, decoded.frame.height), (width, height));
            assert_eq!(decoded.frame.bgra.len(), pixels.len());
            assert!(decoded.is_keyframe);
        }
    }
}

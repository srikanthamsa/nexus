use crate::capture::now_ms;
pub use crate::capture::CaptureOptions;
#[cfg(target_os = "macos")]
use crate::capture::{CapturePipeline, CaptureTick};
#[cfg(target_os = "macos")]
use crate::preview::encode_preview;
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, State};

pub const FRAME_EVENT: &str = "native-capture:frame";
pub const STATS_EVENT: &str = "native-capture:stats";
pub const STOPPED_EVENT: &str = "native-capture:stopped";

const PREVIEW_QUALITY: u8 = 60;
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSummary {
    pub codec: String,
    pub display: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub bitrate_mbps: u32,
    pub started_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub running: bool,
    pub summary: Option<EngineSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FramePayload {
    data: String,
    width: u32,
    height: u32,
    sequence: u64,
    timestamp_ms: u64,
    keyframe: bool,
    encoded_bytes: usize,
    encode_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsPayload {
    sequence: u64,
    capture_fps: f64,
    preview_fps: f64,
    encode_ms: f64,
    preview_ms: f64,
    encoded_kbps: f64,
    preview_kbps: f64,
    width: u32,
    height: u32,
    uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoppedPayload {
    reason: String,
    frames: u64,
    uptime_secs: u64,
    failed: bool,
}

struct ActiveEngine {
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    summary: EngineSummary,
}

#[derive(Default)]
pub struct NativeEngine {
    active: Mutex<Option<ActiveEngine>>,
}

impl NativeEngine {
    fn begin(
        &self,
        summary: EngineSummary,
        stop: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
        handle: JoinHandle<()>,
    ) {
        let mut active = self.active.lock().expect("engine state poisoned");
        *active = Some(ActiveEngine {
            stop,
            running,
            thread: Some(handle),
            summary,
        });
    }

    fn status(&self) -> EngineStatus {
        let active = self.active.lock().expect("engine state poisoned");
        match active.as_ref() {
            Some(engine) => EngineStatus {
                running: engine.running.load(Ordering::Relaxed),
                summary: Some(engine.summary.clone()),
            },
            None => EngineStatus {
                running: false,
                summary: None,
            },
        }
    }

    fn take_active(&self) -> Option<ActiveEngine> {
        self.active.lock().expect("engine state poisoned").take()
    }

    pub fn shutdown(&self) {
        if let Some(engine) = self.take_active() {
            engine.stop.store(true, Ordering::SeqCst);
            engine.running.store(false, Ordering::SeqCst);
            if let Some(handle) = engine.thread {
                let _ = handle.join();
            }
        }
    }
}

struct Reporter {
    app: AppHandle,
    width: u32,
    height: u32,
    started: Instant,
    window_started: Instant,
    sequence: u64,
    frames: u64,
    total_frames: u64,
    previews: u64,
    encode_total_ms: f64,
    encode_samples: u32,
    preview_total_ms: f64,
    encoded_bytes: u64,
    preview_bytes: u64,
}

impl Reporter {
    fn new(app: AppHandle, width: u32, height: u32) -> Self {
        let now = Instant::now();
        Self {
            app,
            width,
            height,
            started: now,
            window_started: now,
            sequence: 0,
            frames: 0,
            total_frames: 0,
            previews: 0,
            encode_total_ms: 0.0,
            encode_samples: 0,
            preview_total_ms: 0.0,
            encoded_bytes: 0,
            preview_bytes: 0,
        }
    }

    fn record_encode(&mut self, sequence: u64, encode_ms: f64, encoded_bytes: usize) {
        self.sequence = sequence;
        self.frames += 1;
        self.total_frames += 1;
        self.encode_total_ms += encode_ms;
        self.encode_samples += 1;
        self.encoded_bytes += encoded_bytes as u64;
    }

    fn record_preview(&mut self, preview_ms: f64, preview_bytes: usize) {
        self.previews += 1;
        self.preview_total_ms += preview_ms;
        self.preview_bytes += preview_bytes as u64;
    }

    fn maybe_emit(&mut self) {
        let elapsed = self.window_started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let seconds = elapsed.as_secs_f64().max(0.001);
        let payload = StatsPayload {
            sequence: self.sequence,
            capture_fps: self.frames as f64 / seconds,
            preview_fps: self.previews as f64 / seconds,
            encode_ms: if self.encode_samples == 0 {
                0.0
            } else {
                self.encode_total_ms / self.encode_samples as f64
            },
            preview_ms: if self.previews == 0 {
                0.0
            } else {
                self.preview_total_ms / self.previews as f64
            },
            encoded_kbps: self.encoded_bytes as f64 * 8.0 / 1000.0 / seconds,
            preview_kbps: self.preview_bytes as f64 * 8.0 / 1000.0 / seconds,
            width: self.width,
            height: self.height,
            uptime_secs: self.started.elapsed().as_secs(),
        };
        let _ = self.app.emit(STATS_EVENT, payload);

        self.window_started = Instant::now();
        self.frames = 0;
        self.previews = 0;
        self.encode_total_ms = 0.0;
        self.encode_samples = 0;
        self.preview_total_ms = 0.0;
        self.encoded_bytes = 0;
        self.preview_bytes = 0;
    }
}

#[tauri::command(async)]
pub fn native_engine_start(
    app: AppHandle,
    engine: State<'_, NativeEngine>,
    options: Option<CaptureOptions>,
) -> Result<EngineSummary, String> {
    let options = options.unwrap_or_default().normalized()?;

    #[cfg(target_os = "macos")]
    {
        engine.shutdown();

        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_app = app.clone();

        let thread_running = Arc::clone(&running);
        let handle = thread::Builder::new()
            .name("parsecplus-capture".to_string())
            .spawn(move || {
                run_engine(worker_app, options, thread_stop, thread_running, ready_tx);
            })
            .map_err(|error| format!("Could not start the capture thread: {error}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(summary)) => {
                engine.begin(summary.clone(), stop, running, handle);
                Ok(summary)
            }
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                Err("Timed out while starting screen capture.".to_string())
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, engine, options);
        Err("Native capture is only supported on macOS.".to_string())
    }
}

#[tauri::command(async)]
pub fn native_engine_stop(engine: State<'_, NativeEngine>) -> Result<(), String> {
    engine.shutdown();
    Ok(())
}

#[tauri::command]
pub fn native_engine_status(engine: State<'_, NativeEngine>) -> EngineStatus {
    engine.status()
}

#[cfg(target_os = "macos")]
fn run_engine(
    app: AppHandle,
    options: CaptureOptions,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<EngineSummary, String>>,
) {
    let mut pipeline = match CapturePipeline::open(&options) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let summary = EngineSummary {
        codec: "HEVC".to_string(),
        display: pipeline.display().to_string(),
        width: options.width,
        height: options.height,
        frame_rate: options.frame_rate,
        bitrate_mbps: options.bitrate_mbps,
        started_at: now_ms(),
    };
    if ready.send(Ok(summary)).is_err() {
        running.store(false, Ordering::SeqCst);
        return;
    }

    let started = Instant::now();
    let preview_interval = Duration::from_secs_f64(1.0 / options.preview_fps);
    let mut last_preview: Option<Instant> = None;
    let mut reporter = Reporter::new(app.clone(), options.width, options.height);
    let mut failure: Option<String> = None;

    while !stop.load(Ordering::Relaxed) {
        let captured = match pipeline.next(STOP_POLL_INTERVAL) {
            Ok(CaptureTick::Frame(captured)) => captured,
            Ok(CaptureTick::Idle) => {
                reporter.maybe_emit();
                continue;
            }
            Ok(CaptureTick::Stopped) => {
                failure = Some(
                    "macOS stopped the screen capture stream. Start the live preview again to resume."
                        .to_string(),
                );
                break;
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        };

        let encoded = &captured.encoded;
        reporter.record_encode(encoded.sequence, encoded.encode_ms, encoded.annex_b.len());

        if last_preview.map_or(true, |last| last.elapsed() >= preview_interval) {
            let preview_started = Instant::now();
            match encode_preview(
                &captured.raw.bgra,
                captured.raw.width,
                captured.raw.height,
                options.preview_max_width,
                PREVIEW_QUALITY,
            ) {
                Ok(preview) => {
                    let preview_ms = preview_started.elapsed().as_secs_f64() * 1000.0;
                    reporter.record_preview(preview_ms, preview.bytes);
                    let payload = FramePayload {
                        data: preview.data_uri,
                        width: preview.width,
                        height: preview.height,
                        sequence: encoded.sequence,
                        timestamp_ms: encoded.timestamp_ms,
                        keyframe: encoded.is_keyframe,
                        encoded_bytes: encoded.annex_b.len(),
                        encode_ms: encoded.encode_ms,
                    };
                    let _ = app.emit(FRAME_EVENT, payload);
                    last_preview = Some(Instant::now());
                }
                Err(_) => {}
            }
        }

        reporter.maybe_emit();
    }

    running.store(false, Ordering::SeqCst);
    drop(pipeline);

    let failed = failure.is_some();
    let reason = failure.unwrap_or_else(|| "Live preview stopped.".to_string());
    let _ = app.emit(
        STOPPED_EVENT,
        StoppedPayload {
            failed,
            reason,
            frames: reporter.total_frames,
            uptime_secs: started.elapsed().as_secs(),
        },
    );
}

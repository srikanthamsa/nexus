use core_graphics_types::geometry::CGSize;
use macos_media_toolkit::BgraFrame;
use metal::{
    foreign_types::{ForeignType, ForeignTypeRef},
    CompileOptions, Device, MTLClearColor, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRegion, MTLStorageMode, MTLStoreAction, MTLTextureUsage, MetalLayer, MetalLayerRef,
    RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState, Texture,
    TextureDescriptor,
};
use objc2::{
    rc::{autoreleasepool, Retained},
    MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{NSView, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CALayer;
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
use tauri::{AppHandle, Emitter, Manager, State};

pub const VIDEO_ERROR_EVENT: &str = "native-video:error";

const TEXTURE_RING: usize = 3;

const SHADER_SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Varying {
    float4 position [[position]];
    float2 uv;
};

vertex Varying video_vertex(uint vid [[vertex_id]], const device float4 *vertices [[buffer(0)]]) {
    float4 corner = vertices[vid];
    Varying result;
    result.position = float4(corner.x, corner.y, 0.0, 1.0);
    result.uv = float2(corner.z, corner.w);
    return result;
}

fragment float4 video_fragment(Varying sample [[stage_in]], texture2d<float> image [[texture(0)]]) {
    constexpr sampler linear(filter::linear, address::clamp_to_edge);
    return image.sample(linear, sample.uv);
}
"#;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VideoBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

impl VideoBounds {
    fn sanitized(self) -> Result<Self, String> {
        if !self.x.is_finite()
            || !self.y.is_finite()
            || !self.width.is_finite()
            || !self.height.is_finite()
            || !self.scale.is_finite()
        {
            return Err("The video surface bounds were invalid.".to_string());
        }
        if self.width < 1.0 || self.height < 1.0 {
            return Err("The video surface has no drawable area.".to_string());
        }
        Ok(Self {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            scale: self.scale.clamp(1.0, 4.0),
        })
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoErrorPayload {
    message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
struct RenderState {
    pending: Option<BgraFrame>,
    size: Option<VideoSize>,
    stop: bool,
}

struct RenderShared {
    state: Mutex<RenderState>,
    signal: Condvar,
}

impl RenderShared {
    fn new() -> Self {
        Self {
            state: Mutex::new(RenderState::default()),
            signal: Condvar::new(),
        }
    }

    fn submit(&self, frame: BgraFrame) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.size = Some(VideoSize {
            width: frame.width,
            height: frame.height,
        });
        if state.stop {
            return;
        }
        state.pending = Some(frame);
        drop(state);
        self.signal.notify_one();
    }

    fn size(&self) -> Option<VideoSize> {
        self.state.lock().ok().and_then(|state| state.size)
    }

    fn stop(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.stop = true;
        state.pending = None;
        state.size = None;
        drop(state);
        self.signal.notify_all();
    }

    fn resume(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stop = false;
        }
    }

    fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.pending = None;
            state.size = None;
        }
    }

    fn take(&self) -> Option<BgraFrame> {
        let mut state = self.state.lock().ok()?;
        loop {
            if state.stop {
                return None;
            }
            if let Some(frame) = state.pending.take() {
                return Some(frame);
            }
            state = self.signal.wait(state).ok()?;
        }
    }
}

#[derive(Default)]
struct SurfaceLifecycle {
    view: Option<usize>,
    layer: Option<usize>,
    thread: Option<JoinHandle<()>>,
}

pub struct VideoSurface {
    shared: Arc<RenderShared>,
    lifecycle: Mutex<SurfaceLifecycle>,
}

impl Default for VideoSurface {
    fn default() -> Self {
        Self {
            shared: Arc::new(RenderShared::new()),
            lifecycle: Mutex::new(SurfaceLifecycle::default()),
        }
    }
}

impl VideoSurface {
    pub fn submit(&self, frame: BgraFrame) {
        self.shared.submit(frame);
    }

    pub fn size(&self) -> Option<VideoSize> {
        self.shared.size()
    }

    pub fn reset(&self) {
        self.shared.reset();
    }

    pub fn attach(&self, app: &AppHandle, bounds: VideoBounds) -> Result<(), String> {
        let bounds = bounds.sanitized()?;
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "The video surface is unavailable.".to_string())?;

        let running = lifecycle
            .thread
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        if running {
            let view = lifecycle.view;
            let layer = lifecycle.layer;
            if let (Some(view), Some(layer)) = (view, layer) {
                return on_main(app, move || {
                    apply_bounds(view, layer, bounds);
                    Ok(())
                });
            }
        }

        if lifecycle.view.is_some() || lifecycle.thread.is_some() {
            self.teardown(app, &mut lifecycle)?;
        }

        let installed = install_view(app, bounds)?;
        self.shared.resume();

        let InstalledView {
            view,
            layer,
            layer_ptr,
        } = installed;
        let shared = Arc::clone(&self.shared);
        let app_for_thread = app.clone();
        let handle = thread::Builder::new()
            .name("parsecplus-video".to_string())
            .spawn(move || {
                if let Err(message) = run_renderer(layer, shared) {
                    let _ = app_for_thread.emit(VIDEO_ERROR_EVENT, VideoErrorPayload { message });
                }
            })
            .map_err(|error| {
                let _ = on_main(app, move || {
                    release_view(view);
                    Ok(())
                });
                format!("Could not start the video renderer: {error}")
            })?;

        lifecycle.view = Some(view);
        lifecycle.layer = Some(layer_ptr);
        lifecycle.thread = Some(handle);
        Ok(())
    }

    pub fn update_bounds(&self, app: &AppHandle, bounds: VideoBounds) -> Result<(), String> {
        let bounds = bounds.sanitized()?;
        let lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "The video surface is unavailable.".to_string())?;
        if let (Some(view), Some(layer)) = (lifecycle.view, lifecycle.layer) {
            on_main(app, move || {
                apply_bounds(view, layer, bounds);
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn detach(&self, app: &AppHandle) -> Result<(), String> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "The video surface is unavailable.".to_string())?;
        self.teardown(app, &mut lifecycle)
    }

    fn teardown(&self, app: &AppHandle, lifecycle: &mut SurfaceLifecycle) -> Result<(), String> {
        self.shared.stop();
        if let Some(handle) = lifecycle.thread.take() {
            let _ = handle.join();
        }
        lifecycle.layer = None;
        if let Some(view) = lifecycle.view.take() {
            on_main(app, move || {
                release_view(view);
                Ok(())
            })?;
        }
        Ok(())
    }
}

struct InstalledView {
    view: usize,
    layer: MetalLayer,
    layer_ptr: usize,
}

fn install_view(app: &AppHandle, bounds: VideoBounds) -> Result<InstalledView, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The main window is not available.".to_string())?;
    let (sender, receiver) = mpsc::sync_channel(1);
    window
        .with_webview(move |platform| {
            let _ = sender.send(install_below_webview(platform.inner(), bounds));
        })
        .map_err(|error| format!("Could not reach the webview: {error}"))?;
    receiver
        .recv()
        .map_err(|_| "The webview did not complete the video surface install.".to_string())?
}

fn install_below_webview(
    webview: *mut c_void,
    bounds: VideoBounds,
) -> Result<InstalledView, String> {
    if webview.is_null() {
        return Err("The webview is not ready for video.".to_string());
    }
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "The video surface must be created on the main thread.".to_string())?;
    let webview: &NSView = unsafe { &*(webview as *const NSView) };
    let parent = unsafe { webview.superview() }
        .ok_or_else(|| "The webview is not attached to a window.".to_string())?;

    let frame = view_frame(&parent, bounds);
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);

    let layer = MetalLayer::new();
    layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    layer.set_framebuffer_only(true);
    layer.set_maximum_drawable_count(TEXTURE_RING as u64);
    layer.set_contents_scale(bounds.scale);
    layer.set_drawable_size(drawable_size(bounds));

    let layer_ptr = layer.as_ptr() as *mut c_void;
    let calayer: &CALayer = unsafe { &*(layer_ptr as *const CALayer) };
    view.setWantsLayer(true);
    view.setLayer(Some(calayer));

    parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, Some(webview));

    Ok(InstalledView {
        view: Retained::into_raw(view) as *mut c_void as usize,
        layer,
        layer_ptr: layer_ptr as usize,
    })
}

fn view_frame(parent: &NSView, bounds: VideoBounds) -> NSRect {
    let parent_height = parent.bounds().size.height;
    let origin_y = if parent.isFlipped() {
        bounds.y
    } else {
        parent_height - bounds.y - bounds.height
    };
    NSRect {
        origin: NSPoint {
            x: bounds.x,
            y: origin_y,
        },
        size: NSSize {
            width: bounds.width,
            height: bounds.height,
        },
    }
}

fn drawable_size(bounds: VideoBounds) -> CGSize {
    CGSize::new(
        (bounds.width * bounds.scale).round(),
        (bounds.height * bounds.scale).round(),
    )
}

fn apply_bounds(view: usize, layer: usize, bounds: VideoBounds) {
    let view: &NSView = unsafe { &*(view as *const NSView) };
    let Some(parent) = (unsafe { view.superview() }) else {
        return;
    };
    view.setFrame(view_frame(&parent, bounds));

    let layer: &MetalLayerRef = unsafe { MetalLayerRef::from_ptr(layer as *mut _) };
    layer.set_contents_scale(bounds.scale);
    layer.set_drawable_size(drawable_size(bounds));
}

fn release_view(view: usize) {
    let ptr = view as *mut NSView;
    let view: &NSView = unsafe { &*ptr };
    view.removeFromSuperview();
    drop(unsafe { Retained::from_raw(ptr) });
}

fn on_main<T, F>(app: &AppHandle, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(task());
    })
    .map_err(|error| format!("Could not reach the main thread: {error}"))?;
    receiver
        .recv()
        .map_err(|_| "The main thread did not complete the video operation.".to_string())?
}

fn build_pipeline(device: &Device) -> Result<RenderPipelineState, String> {
    let library = device.new_library_with_source(SHADER_SOURCE, &CompileOptions::new())?;
    let vertex = library.get_function("video_vertex", None)?;
    let fragment = library.get_function("video_fragment", None)?;

    let descriptor = RenderPipelineDescriptor::new();
    descriptor.set_vertex_function(Some(&vertex));
    descriptor.set_fragment_function(Some(&fragment));
    descriptor
        .color_attachments()
        .object_at(0)
        .ok_or_else(|| "Metal did not expose a color attachment.".to_string())?
        .set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    device.new_render_pipeline_state(&descriptor)
}

fn run_renderer(layer: MetalLayer, shared: Arc<RenderShared>) -> Result<(), String> {
    let device = Device::system_default()
        .ok_or_else(|| "Metal is not available on this Mac.".to_string())?;
    let queue = device.new_command_queue();
    let pipeline = build_pipeline(&device)?;
    layer.set_device(&device);

    let mut textures: Vec<Texture> = Vec::with_capacity(TEXTURE_RING);
    let mut texture_width = 0u64;
    let mut texture_height = 0u64;
    let mut texture_index = 0usize;

    while let Some(frame) = shared.take() {
        let width = u64::from(frame.width);
        let height = u64::from(frame.height);
        if width == 0 || height == 0 {
            continue;
        }
        let expected = frame.width as usize * frame.height as usize * 4;
        if frame.bgra.len() < expected {
            continue;
        }

        if width != texture_width || height != texture_height {
            textures.clear();
            for _ in 0..TEXTURE_RING {
                textures.push(create_texture(&device, width, height));
            }
            texture_width = width;
            texture_height = height;
            texture_index = 0;
        }

        let texture = &textures[texture_index];
        texture_index = (texture_index + 1) % TEXTURE_RING;
        texture.replace_region(
            MTLRegion::new_2d(0, 0, width, height),
            0,
            frame.bgra.as_ptr().cast(),
            width * 4,
        );

        autoreleasepool(|_| {
            let Some(drawable) = layer.next_drawable() else {
                return;
            };
            let target = drawable.texture();
            let target_width = target.width() as f64;
            let target_height = target.height() as f64;
            if target_width <= 0.0 || target_height <= 0.0 {
                return;
            }

            let vertices = fit_vertices(
                f64::from(frame.width),
                f64::from(frame.height),
                target_width,
                target_height,
            );

            let pass = RenderPassDescriptor::new();
            let Some(attachment) = pass.color_attachments().object_at(0) else {
                return;
            };
            attachment.set_texture(Some(target));
            attachment.set_load_action(MTLLoadAction::Clear);
            attachment.set_store_action(MTLStoreAction::Store);
            attachment.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 1.0));

            let command_buffer = queue.new_command_buffer();
            let encoder = command_buffer.new_render_command_encoder(pass);
            encoder.set_render_pipeline_state(&pipeline);
            encoder.set_vertex_bytes(
                0,
                std::mem::size_of_val(&vertices) as u64,
                vertices.as_ptr().cast(),
            );
            encoder.set_fragment_texture(0, Some(texture));
            encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
            encoder.end_encoding();
            command_buffer.present_drawable(drawable);
            command_buffer.commit();
        });
    }

    Ok(())
}

fn create_texture(device: &Device, width: u64, height: u64) -> Texture {
    let descriptor = TextureDescriptor::new();
    descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    descriptor.set_width(width);
    descriptor.set_height(height);
    descriptor.set_usage(MTLTextureUsage::ShaderRead);
    descriptor.set_storage_mode(MTLStorageMode::Shared);
    device.new_texture(&descriptor)
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
struct Vertex([f32; 4]);

fn fit_vertices(
    source_width: f64,
    source_height: f64,
    target_width: f64,
    target_height: f64,
) -> [Vertex; 6] {
    let (extent_x, extent_y) = if source_width <= 0.0
        || source_height <= 0.0
        || target_width <= 0.0
        || target_height <= 0.0
    {
        (1.0, 1.0)
    } else {
        let source_aspect = source_width / source_height;
        let target_aspect = target_width / target_height;
        if source_aspect > target_aspect {
            (1.0, (target_aspect / source_aspect) as f32)
        } else {
            ((source_aspect / target_aspect) as f32, 1.0)
        }
    };

    [
        Vertex([-extent_x, extent_y, 0.0, 0.0]),
        Vertex([-extent_x, -extent_y, 0.0, 1.0]),
        Vertex([extent_x, extent_y, 1.0, 0.0]),
        Vertex([extent_x, extent_y, 1.0, 0.0]),
        Vertex([-extent_x, -extent_y, 0.0, 1.0]),
        Vertex([extent_x, -extent_y, 1.0, 1.0]),
    ]
}

#[tauri::command(async)]
pub fn native_video_attach(app: AppHandle, bounds: VideoBounds) -> Result<(), String> {
    app.state::<VideoSurface>().attach(&app, bounds)
}

#[tauri::command(async)]
pub fn native_video_bounds(app: AppHandle, bounds: VideoBounds) -> Result<(), String> {
    app.state::<VideoSurface>().update_bounds(&app, bounds)
}

#[tauri::command(async)]
pub fn native_video_detach(app: AppHandle) -> Result<(), String> {
    app.state::<VideoSurface>().detach(&app)
}

#[tauri::command]
pub fn native_video_size(surface: State<'_, VideoSurface>) -> Option<VideoSize> {
    surface.size()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_vertices_letterboxes_wide_source() {
        let vertices = fit_vertices(2.0, 1.0, 4.0, 4.0);
        assert_eq!(vertices[0].0, [-1.0, 0.5, 0.0, 0.0]);
        assert_eq!(vertices[5].0, [1.0, -0.5, 1.0, 1.0]);
    }

    #[test]
    fn fit_vertices_pillarboxes_tall_source() {
        let vertices = fit_vertices(1.0, 2.0, 4.0, 4.0);
        assert_eq!(vertices[0].0, [-0.5, 1.0, 0.0, 0.0]);
        assert_eq!(vertices[5].0, [0.5, -1.0, 1.0, 1.0]);
    }

    #[test]
    fn metal_draws_cpu_frames_into_letterboxed_target() {
        let Some(device) = Device::system_default() else {
            return;
        };
        let queue = device.new_command_queue();
        let pipeline = build_pipeline(&device).expect("shader pipeline");

        let source_desc = TextureDescriptor::new();
        source_desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        source_desc.set_width(4);
        source_desc.set_height(2);
        source_desc.set_usage(MTLTextureUsage::ShaderRead);
        source_desc.set_storage_mode(MTLStorageMode::Shared);
        let source = device.new_texture(&source_desc);
        let pixels: [u8; 32] = [
            0, 0, 255, 255, 0, 0, 255, 255, 0, 255, 0, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 0,
            0, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ];
        source.replace_region(MTLRegion::new_2d(0, 0, 4, 2), 0, pixels.as_ptr().cast(), 16);

        let target_desc = TextureDescriptor::new();
        target_desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        target_desc.set_width(4);
        target_desc.set_height(4);
        target_desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        target_desc.set_storage_mode(MTLStorageMode::Shared);
        let target = device.new_texture(&target_desc);

        let pass = RenderPassDescriptor::new();
        let attachment = pass
            .color_attachments()
            .object_at(0)
            .expect("color attachment");
        attachment.set_texture(Some(&target));
        attachment.set_load_action(MTLLoadAction::Clear);
        attachment.set_store_action(MTLStoreAction::Store);
        attachment.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 1.0));

        let vertices = fit_vertices(4.0, 2.0, 4.0, 4.0);
        let command_buffer = queue.new_command_buffer();
        let encoder = command_buffer.new_render_command_encoder(pass);
        encoder.set_render_pipeline_state(&pipeline);
        encoder.set_vertex_bytes(
            0,
            std::mem::size_of_val(&vertices) as u64,
            vertices.as_ptr().cast(),
        );
        encoder.set_fragment_texture(0, Some(&source));
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
        encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();

        let mut readback = vec![0u8; 4 * 4 * 4];
        target.get_bytes(
            readback.as_mut_ptr().cast(),
            16,
            MTLRegion::new_2d(0, 0, 4, 4),
            0,
        );
        let pixel = |x: usize, y: usize| {
            let offset = (y * 4 + x) * 4;
            [
                readback[offset],
                readback[offset + 1],
                readback[offset + 2],
                readback[offset + 3],
            ]
        };

        assert_eq!(pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(3, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(0, 3), [0, 0, 0, 255]);
        assert_eq!(pixel(0, 1), [0, 0, 255, 255]);
        assert_eq!(pixel(1, 1), [0, 0, 255, 255]);
        assert_eq!(pixel(2, 1), [0, 255, 0, 255]);
        assert_eq!(pixel(3, 1), [0, 255, 0, 255]);
        assert_eq!(pixel(0, 2), [255, 0, 0, 255]);
        assert_eq!(pixel(1, 2), [255, 0, 0, 255]);
        assert_eq!(pixel(2, 2), [255, 255, 255, 255]);
        assert_eq!(pixel(3, 2), [255, 255, 255, 255]);
    }
}

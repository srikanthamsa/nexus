use crate::gamepad::GamepadState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputStatus {
    pub armed: bool,
    pub accessibility: bool,
    pub platform_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteInput {
    Pointer {
        x: f64,
        y: f64,
    },
    Button {
        button: String,
        pressed: bool,
    },
    Scroll {
        delta_x: f64,
        delta_y: f64,
    },
    Text {
        text: String,
    },
    Key {
        key_code: u16,
        pressed: bool,
        #[serde(default)]
        flags: Vec<String>,
    },
    /// A controller snapshot. The host routes this to the virtual gamepad
    /// backend instead of the CGEvent input backend.
    Gamepad {
        state: GamepadState,
    },
}

#[derive(Default)]
struct InputState {
    armed: bool,
    pressed: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct NativeInput {
    state: Arc<Mutex<InputState>>,
}

impl NativeInput {
    fn require_armed(&self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if unsafe { crate::mac_permissions::AXIsProcessTrusted() } == 0 {
                let was_armed = {
                    let mut state = self.state.lock().expect("input state poisoned");
                    let was_armed = state.armed;
                    state.armed = false;
                    was_armed
                };
                if was_armed {
                    self.release_all();
                }
                return Err(
                    "Accessibility access is required for native input. Enable Nexus in \
                     System Settings → Privacy & Security → Accessibility."
                        .to_string(),
                );
            }
            let state = self.state.lock().expect("input state poisoned");
            if !state.armed {
                return Err("Native input is not armed.".to_string());
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Native input is only supported on macOS.".to_string())
        }
    }

    pub fn status(&self) -> InputStatus {
        #[cfg(target_os = "macos")]
        {
            let armed = self.state.lock().expect("input state poisoned").armed;
            InputStatus {
                armed,
                accessibility: unsafe { crate::mac_permissions::AXIsProcessTrusted() != 0 },
                platform_supported: true,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            InputStatus {
                armed: false,
                accessibility: false,
                platform_supported: false,
            }
        }
    }

    pub fn set_armed(&self, armed: bool) -> Result<InputStatus, String> {
        #[cfg(target_os = "macos")]
        {
            if armed {
                if unsafe { crate::mac_permissions::AXIsProcessTrusted() } == 0 {
                    return Err(
                        "Accessibility access is required before input can be armed. Enable \
                         Nexus in System Settings → Privacy & Security → Accessibility."
                            .to_string(),
                    );
                }
                self.state.lock().expect("input state poisoned").armed = true;
            } else {
                self.release_all();
            }
            Ok(self.status())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = armed;
            Err("Native input is only supported on macOS.".to_string())
        }
    }

    pub fn release_all(&self) {
        #[cfg(target_os = "macos")]
        {
            let pressed = {
                let mut state = self.state.lock().expect("input state poisoned");
                state.armed = false;
                std::mem::take(&mut state.pressed)
            };
            for button in pressed {
                let _ = platform::post_button(button, false);
            }
        }
    }

    /// Applies a remote event through the same Accessibility + explicit-arm gate
    /// as local commands. Returns an error when the session is not armed.
    pub fn apply(&self, event: &RemoteInput) -> Result<(), String> {
        self.require_armed()?;
        #[cfg(target_os = "macos")]
        {
            match event {
                RemoteInput::Pointer { x, y } => platform::post_pointer(*x, *y),
                RemoteInput::Button { button, pressed } => {
                    let code = match button.as_str() {
                        "left" => 0,
                        "right" => 1,
                        "middle" => 2,
                        other => return Err(format!("Unknown mouse button: {other}")),
                    };
                    platform::post_button(code, *pressed)?;
                    let mut state = self.state.lock().expect("input state poisoned");
                    if *pressed {
                        if !state.pressed.contains(&code) {
                            state.pressed.push(code);
                        }
                    } else {
                        state.pressed.retain(|value| *value != code);
                    }
                    Ok(())
                }
                RemoteInput::Scroll { delta_x, delta_y } => {
                    platform::post_scroll(*delta_x, *delta_y)
                }
                RemoteInput::Text { text } => platform::post_text(text),
                RemoteInput::Key {
                    key_code,
                    pressed,
                    flags,
                } => platform::post_key(*key_code, *pressed, flags),
                RemoteInput::Gamepad { .. } => {
                    Err("Gamepad events are handled by the host gamepad backend.".to_string())
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = event;
            Err("Native input is only supported on macOS.".to_string())
        }
    }

    pub fn shutdown(&self) {
        self.release_all();
    }
}

#[tauri::command]
pub fn native_input_arm(input: State<'_, NativeInput>, armed: bool) -> Result<InputStatus, String> {
    input.set_armed(armed)
}

#[tauri::command]
pub fn native_input_status(input: State<'_, NativeInput>) -> InputStatus {
    input.status()
}

#[tauri::command]
pub fn native_input_pointer(input: State<'_, NativeInput>, x: f64, y: f64) -> Result<(), String> {
    input.apply(&RemoteInput::Pointer { x, y })
}

#[tauri::command]
pub fn native_input_button(
    input: State<'_, NativeInput>,
    button: String,
    pressed: bool,
) -> Result<(), String> {
    input.apply(&RemoteInput::Button { button, pressed })
}

#[tauri::command]
pub fn native_input_scroll(
    input: State<'_, NativeInput>,
    delta_x: f64,
    delta_y: f64,
) -> Result<(), String> {
    input.apply(&RemoteInput::Scroll { delta_x, delta_y })
}

#[tauri::command]
pub fn native_input_text(input: State<'_, NativeInput>, text: String) -> Result<(), String> {
    input.apply(&RemoteInput::Text { text })
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{ffi::c_void, ptr};

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *mut c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CGEventCreateMouseEvent(
            source: *mut c_void,
            mouse_type: u32,
            mouse_cursor_position: CGPoint,
            mouse_button: u32,
        ) -> *mut c_void;
        fn CGEventCreateScrollWheelEvent2(
            source: *mut c_void,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
            wheel2: i32,
            wheel3: i32,
        ) -> *mut c_void;
        fn CGEventCreateKeyboardEvent(
            source: *mut c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> *mut c_void;
        fn CGEventKeyboardSetUnicodeString(event: *mut c_void, length: usize, string: *const u16);
        fn CGEventSetFlags(event: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut c_void);
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGMainDisplayID() -> u32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *mut c_void);
    }

    const HID_TAP: u32 = 0;
    const MOUSE_MOVED: u32 = 5;
    const SCROLL_PIXEL: u32 = 0;

    const FLAG_SHIFT: u64 = 1 << 17;
    const FLAG_CONTROL: u64 = 1 << 18;
    const FLAG_OPTION: u64 = 1 << 19;
    const FLAG_COMMAND: u64 = 1 << 20;

    struct Event(*mut c_void);

    impl Event {
        fn new(pointer: *mut c_void) -> Result<Self, String> {
            if pointer.is_null() {
                Err("Could not create the native input event.".to_string())
            } else {
                Ok(Self(pointer))
            }
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn map_point(x: f64, y: f64, bounds: CGRect) -> Result<CGPoint, String> {
        if !x.is_finite() || !y.is_finite() {
            return Err("Pointer coordinates must be finite.".to_string());
        }
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return Err("Pointer coordinates must be normalized between 0 and 1.".to_string());
        }
        Ok(CGPoint {
            x: bounds.origin.x + bounds.size.width * x,
            y: bounds.origin.y + bounds.size.height * y,
        })
    }

    fn normalized_point(x: f64, y: f64) -> Result<CGPoint, String> {
        let bounds = unsafe { CGDisplayBounds(CGMainDisplayID()) };
        map_point(x, y, bounds)
    }

    pub fn post_pointer(x: f64, y: f64) -> Result<(), String> {
        let point = normalized_point(x, y)?;
        let event =
            Event::new(unsafe { CGEventCreateMouseEvent(ptr::null_mut(), MOUSE_MOVED, point, 0) })?;
        unsafe { CGEventPost(HID_TAP, event.0) };
        Ok(())
    }

    fn current_location() -> Result<CGPoint, String> {
        let event = Event::new(unsafe { CGEventCreate(ptr::null_mut()) })?;
        Ok(unsafe { CGEventGetLocation(event.0) })
    }

    pub fn post_button(button: u8, pressed: bool) -> Result<(), String> {
        let (down_type, up_type, cg_button) = match button {
            0 => (1, 2, 0),
            1 => (3, 4, 1),
            2 => (25, 26, 2),
            other => return Err(format!("Unknown mouse button code: {other}")),
        };
        let point = current_location()?;
        let event_type = if pressed { down_type } else { up_type };
        let event = Event::new(unsafe {
            CGEventCreateMouseEvent(ptr::null_mut(), event_type, point, cg_button)
        })?;
        unsafe { CGEventPost(HID_TAP, event.0) };
        Ok(())
    }

    pub fn post_scroll(delta_x: f64, delta_y: f64) -> Result<(), String> {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return Err("Scroll deltas must be finite.".to_string());
        }
        let vertical = (-delta_y).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        let horizontal = (-delta_x).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        let event = Event::new(unsafe {
            CGEventCreateScrollWheelEvent2(
                ptr::null_mut(),
                SCROLL_PIXEL,
                2,
                vertical,
                horizontal,
                0,
            )
        })?;
        unsafe { CGEventPost(HID_TAP, event.0) };
        Ok(())
    }

    pub fn post_text(text: &str) -> Result<(), String> {
        let units: Vec<u16> = text.encode_utf16().collect();
        if units.len() > 4096 {
            return Err("Text input is limited to 4096 UTF-16 code units per call.".to_string());
        }
        if units.is_empty() {
            return Ok(());
        }

        for chunk in units.chunks(64) {
            let down = Event::new(unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), 0, true) })?;
            unsafe { CGEventKeyboardSetUnicodeString(down.0, chunk.len(), chunk.as_ptr()) };
            unsafe { CGEventPost(HID_TAP, down.0) };

            let up = Event::new(unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), 0, false) })?;
            unsafe { CGEventPost(HID_TAP, up.0) };
        }
        Ok(())
    }

    fn modifier_flags(flags: &[String]) -> Result<u64, String> {
        let mut value = 0u64;
        for flag in flags {
            value |= match flag.as_str() {
                "shift" => FLAG_SHIFT,
                "control" => FLAG_CONTROL,
                "option" => FLAG_OPTION,
                "command" => FLAG_COMMAND,
                other => return Err(format!("Unknown keyboard modifier: {other}")),
            };
        }
        Ok(value)
    }

    pub fn post_key(key_code: u16, pressed: bool, flags: &[String]) -> Result<(), String> {
        if key_code > 0x7f {
            return Err("Unsupported virtual key code.".to_string());
        }
        let flag_value = modifier_flags(flags)?;
        let event =
            Event::new(unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), key_code, pressed) })?;
        if flag_value != 0 {
            unsafe { CGEventSetFlags(event.0, flag_value) };
        }
        unsafe { CGEventPost(HID_TAP, event.0) };
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{map_point, modifier_flags, CGPoint, CGRect, CGSize};

        const BOUNDS: CGRect = CGRect {
            origin: CGPoint { x: -100.0, y: 50.0 },
            size: CGSize {
                width: 1440.0,
                height: 900.0,
            },
        };

        #[test]
        fn maps_normalized_coordinates_into_display_bounds() {
            let top_left = map_point(0.0, 0.0, BOUNDS).expect("top left");
            assert_eq!((top_left.x, top_left.y), (-100.0, 50.0));

            let middle = map_point(0.5, 0.5, BOUNDS).expect("middle");
            assert_eq!((middle.x, middle.y), (620.0, 500.0));

            let bottom_right = map_point(1.0, 1.0, BOUNDS).expect("bottom right");
            assert_eq!((bottom_right.x, bottom_right.y), (1340.0, 950.0));
        }

        #[test]
        fn rejects_out_of_range_and_non_finite_coordinates() {
            assert!(map_point(-0.1, 0.5, BOUNDS).is_err());
            assert!(map_point(0.5, 1.1, BOUNDS).is_err());
            assert!(map_point(f64::NAN, 0.5, BOUNDS).is_err());
            assert!(map_point(0.5, f64::INFINITY, BOUNDS).is_err());
        }

        #[test]
        fn maps_modifier_names_and_rejects_unknown_flags() {
            let flags = modifier_flags(&["shift".to_string(), "command".to_string()]).unwrap();
            assert_eq!(flags, (1 << 17) | (1 << 20));
            assert_eq!(modifier_flags(&[]).unwrap(), 0);
            assert!(modifier_flags(&["hyper".to_string()]).is_err());
        }
    }
}

//! Controller (gamepad) passthrough.
//!
//! The client samples a physical controller and sends a normalized snapshot
//! over the encrypted control channel. The host turns that snapshot into a
//! virtual HID gamepad so games see a real controller.
//!
//! macOS only exposes virtual HID devices to processes signed with the
//! restricted `com.apple.developer.hid.virtual.device` entitlement. When the
//! entitlement is missing, [`VirtualGamepad::new`] returns an actionable error
//! instead of silently dropping input.

use serde::{Deserialize, Serialize};

/// Number of analog axes carried on the wire.
pub const MAX_AXES: usize = 8;
/// Number of digital buttons carried on the wire. W3C standard mapping has
/// 17 (A/B/X/Y, shoulders, triggers, Back/Start, stick clicks, D-pad, Guide).
pub const MAX_BUTTONS: usize = 17;
/// Report ID + 2 button bytes + hat + 2 trigger bytes + four 16-bit stick axes.
pub const REPORT_LEN: usize = 14;

/// A normalized snapshot of one physical controller.
///
/// `axes` are in `-1.0..=1.0` (sticks) and `buttons` are in `0.0..=1.0`
/// (triggers are analog, face buttons are 0 or 1). The layout follows the
/// W3C Gamepad API standard mapping so the client can forward it unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GamepadState {
    /// Controller slot on the client, used to keep multiple pads apart.
    pub slot: u8,
    #[serde(default)]
    pub name: String,
    /// `false` releases the virtual device for this slot.
    pub connected: bool,
    #[serde(default)]
    pub axes: Vec<f64>,
    #[serde(default)]
    pub buttons: Vec<f64>,
}

impl GamepadState {
    /// Clamps and truncates a snapshot so a malformed peer cannot send
    /// out-of-range values or an unbounded payload into the HID report.
    pub fn normalized(&self) -> Self {
        let axes = self
            .axes
            .iter()
            .take(MAX_AXES)
            .map(|value| {
                if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        let buttons = self
            .buttons
            .iter()
            .take(MAX_BUTTONS)
            .map(|value| {
                if value.is_finite() {
                    value.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        Self {
            slot: self.slot,
            name: self.name.chars().take(64).collect(),
            connected: self.connected,
            axes,
            buttons,
        }
    }

    fn axis(&self, index: usize) -> f64 {
        self.axes.get(index).copied().unwrap_or(0.0)
    }

    fn button(&self, index: usize) -> f64 {
        self.buttons.get(index).copied().unwrap_or(0.0)
    }

    fn pressed(&self, index: usize) -> bool {
        self.button(index) >= 0.5
    }

    /// Left/right triggers. The standard mapping exposes them as buttons 6/7;
    /// some controllers only report them as axes 4/5, so fall back to those.
    fn trigger(&self, button_index: usize, axis_index: usize) -> f64 {
        if self.buttons.len() > button_index {
            self.button(button_index)
        } else {
            self.axis(axis_index).max(0.0)
        }
    }

    /// D-pad hat switch: 0 = up, 1 = up-right, … 7 = up-left, 0x0F = neutral.
    fn hat(&self) -> u8 {
        let up = self.pressed(12);
        let down = self.pressed(13);
        let left = self.pressed(14);
        let right = self.pressed(15);
        match (up, right, down, left) {
            (true, false, false, false) => 0,
            (true, true, false, false) => 1,
            (false, true, false, false) => 2,
            (false, true, true, false) => 3,
            (false, false, true, false) => 4,
            (false, false, true, true) => 5,
            (false, false, false, true) => 6,
            (true, false, false, true) => 7,
            _ => 0x0F,
        }
    }

    /// Maps W3C standard button indices to the Xbox-style HID button field.
    ///
    /// Xbox/Microsoft HID layout is not the same as the W3C order:
    /// bit 0..3 A/B/X/Y, bit 4/5 LB/RB, bit 6/7 Back/Start, bit 8/9 L3/R3,
    /// bit 10 Guide. Triggers and the D-pad are separate fields (trigger
    /// bytes and the hat switch), so they are deliberately not in this mask.
    fn button_bits(&self) -> u16 {
        let mappings = [
            (0usize, 0u16), // A
            (1, 1),         // B
            (2, 2),         // X
            (3, 3),         // Y
            (4, 4),         // LB / L1
            (5, 5),         // RB / R1
            (8, 6),         // Back / View / Share
            (9, 7),         // Start / Menu / Options
            (10, 8),        // LS / L3
            (11, 9),        // RS / R3
            (16, 10),       // Guide / Home / PS
        ];
        let mut bits = 0u16;
        for (button, bit) in mappings {
            if self.pressed(button) {
                bits |= 1 << bit;
            }
        }
        bits
    }

    /// Encodes the snapshot as the 14-byte Xbox-style input report described
    /// by [`REPORT_DESCRIPTOR`].
    pub fn report(&self) -> [u8; REPORT_LEN] {
        let mut report = [0u8; REPORT_LEN];
        report[0] = 0x01;
        let bits = self.button_bits();
        report[1] = (bits & 0xFF) as u8;
        report[2] = (bits >> 8) as u8;
        report[3] = self.hat();
        report[4] = trigger_byte(self.trigger(6, 4));
        report[5] = trigger_byte(self.trigger(7, 5));
        write_axis(&mut report, 6, axis_short(self.axis(0)));
        write_axis(&mut report, 8, axis_short(self.axis(1)));
        write_axis(&mut report, 10, axis_short(self.axis(2)));
        write_axis(&mut report, 12, axis_short(self.axis(3)));
        report
    }
}

fn axis_short(value: f64) -> i16 {
    (value.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn write_axis(report: &mut [u8; REPORT_LEN], offset: usize, value: i16) {
    let bytes = value.to_le_bytes();
    report[offset] = bytes[0];
    report[offset + 1] = bytes[1];
}

fn trigger_byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// HID report descriptor for an Xbox-style gamepad.
///
/// macOS GameController/SDL/Steam recognise this layout: 16 buttons, a 4-bit
/// hat with padding, two 8-bit triggers, and four signed 16-bit stick axes.
/// The report byte order is buttons, hat, triggers, axes — matching the
/// struct byte order, not the W3C order.
pub const REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x04, // Usage (Joystick)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x01, //   Report ID (1)
    0x05, 0x09, //   Usage Page (Button)
    0x19, 0x01, //   Usage Minimum (1)
    0x29, 0x10, //   Usage Maximum (16)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x95, 0x10, //   Report Count (16)
    0x75, 0x01, //   Report Size (1)
    0x81, 0x02, //   Input (Data,Var,Abs)
    0x05, 0x01, //   Usage Page (Generic Desktop)
    0x09, 0x39, //   Usage (Hat switch)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x07, //   Logical Maximum (7)
    0x35, 0x00, //   Physical Minimum (0)
    0x46, 0x3B, 0x01, // Physical Maximum (315)
    0x65, 0x14, //   Unit (Eng Rot: Degree)
    0x75, 0x04, //   Report Size (4)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x42, //   Input (Data,Var,Abs,Null State)
    0x75, 0x04, //   Report Size (4)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x01, //   Input (Const)
    0x05, 0x02, //   Usage Page (Simulation Controls)
    0x09, 0xC5, //   Usage (Brake) - left trigger
    0x09, 0xC4, //   Usage (Accelerator) - right trigger
    0x15, 0x00, //   Logical Minimum (0)
    0x26, 0xFF, 0x00, // Logical Maximum (255)
    0x75, 0x08, //   Report Size (8)
    0x95, 0x02, //   Report Count (2)
    0x81, 0x02, //   Input (Data,Var,Abs)
    0x05, 0x01, //   Usage Page (Generic Desktop)
    0x09, 0x30, //   Usage (X) - left stick X
    0x09, 0x31, //   Usage (Y) - left stick Y
    0x09, 0x32, //   Usage (Z) - right stick X
    0x09, 0x35, //   Usage (Rz) - right stick Y
    0x16, 0x00, 0x80, // Logical Minimum (-32768)
    0x26, 0xFF, 0x7F, // Logical Maximum (32767)
    0x75, 0x10, //   Report Size (16)
    0x95, 0x04, //   Report Count (4)
    0x81, 0x02, //   Input (Data,Var,Abs)
    0xC0, // End Collection
];

#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;

/// A virtual HID gamepad backed by `IOHIDUserDevice`.
#[cfg(target_os = "macos")]
pub struct VirtualGamepad {
    device: core_foundation::base::CFType,
}

#[cfg(target_os = "macos")]
impl VirtualGamepad {
    /// Creates a virtual gamepad. Fails with an actionable message when the
    /// process is not signed with the virtual-HID entitlement.
    pub fn new(slot: u8) -> Result<Self, String> {
        use core_foundation::base::{CFType, TCFType};
        use core_foundation::data::CFData;
        use core_foundation::dictionary::CFMutableDictionary;
        use core_foundation::number::CFNumber;
        use core_foundation::string::CFString;

        let descriptor = CFData::from_buffer(&REPORT_DESCRIPTOR);
        let mut properties: CFMutableDictionary<CFString, CFType> = CFMutableDictionary::new();
        properties.add(&CFString::from("ReportDescriptor"), &descriptor.as_CFType());
        properties.add(
            &CFString::from("Product"),
            &CFString::from(format!("ParsecPlus Virtual Gamepad {slot}").as_str()).as_CFType(),
        );
        properties.add(
            &CFString::from("Manufacturer"),
            &CFString::from("ParsecPlus").as_CFType(),
        );
        properties.add(
            &CFString::from("Transport"),
            &CFString::from("USB").as_CFType(),
        );
        properties.add(
            &CFString::from("VendorID"),
            &CFNumber::from(0x1209i32).as_CFType(),
        );
        properties.add(
            &CFString::from("ProductID"),
            &CFNumber::from(0x5853i32 + i32::from(slot)).as_CFType(),
        );
        properties.add(
            &CFString::from("VersionNumber"),
            &CFNumber::from(1i32).as_CFType(),
        );
        properties.add(
            &CFString::from("PrimaryUsagePage"),
            &CFNumber::from(0x01i32).as_CFType(),
        );
        properties.add(
            &CFString::from("PrimaryUsage"),
            &CFNumber::from(0x04i32).as_CFType(),
        );

        let raw =
            unsafe { IOHIDUserDeviceCreate(std::ptr::null(), properties.as_concrete_TypeRef()) };
        if raw.is_null() {
            return Err(
                "macOS refused to create a virtual gamepad. The host app must be signed with \
                 the restricted com.apple.developer.hid.virtual.device entitlement."
                    .to_string(),
            );
        }
        let device = unsafe { CFType::wrap_under_create_rule(raw) };
        Ok(Self { device })
    }

    /// Sends one input report to the virtual device.
    pub fn apply(&self, state: &GamepadState) -> Result<(), String> {
        let report = state.normalized().report();
        let result = unsafe {
            IOHIDUserDeviceHandleReport(
                self.device.as_CFTypeRef(),
                report.as_ptr(),
                report.len() as isize,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "The virtual gamepad rejected an input report (0x{result:x})."
            ))
        }
    }
}

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDUserDeviceCreate(
        allocator: core_foundation::base::CFAllocatorRef,
        properties: core_foundation::dictionary::CFDictionaryRef,
    ) -> core_foundation::base::CFTypeRef;
    fn IOHIDUserDeviceHandleReport(
        device: core_foundation::base::CFTypeRef,
        report: *const u8,
        report_length: core_foundation::base::CFIndex,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(axes: &[f64], buttons: &[f64]) -> GamepadState {
        GamepadState {
            slot: 0,
            name: "test".to_string(),
            connected: true,
            axes: axes.to_vec(),
            buttons: buttons.to_vec(),
        }
    }

    #[test]
    fn centers_sticks_and_releases_triggers() {
        let report = state(&[0.0, 0.0, 0.0, 0.0], &[]).report();
        assert_eq!(report[0], 0x01);
        assert_eq!(&report[1..3], &[0, 0]);
        assert_eq!(report[3], 0x0F);
        assert_eq!(&report[4..6], &[0, 0]);
        assert_eq!(&report[6..14], &[0; 8]);
    }

    #[test]
    fn maps_stick_extremes_and_inverts_nothing() {
        let report = state(&[-1.0, 1.0, -1.0, 1.0], &[]).report();
        assert_eq!(i16::from_le_bytes([report[6], report[7]]), -32767);
        assert_eq!(i16::from_le_bytes([report[8], report[9]]), 32767);
        assert_eq!(i16::from_le_bytes([report[10], report[11]]), -32767);
        assert_eq!(i16::from_le_bytes([report[12], report[13]]), 32767);
    }

    #[test]
    fn maps_triggers_from_buttons_and_axes() {
        let from_buttons = state(&[], &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.5]).report();
        assert_eq!(from_buttons[4], 255);
        assert_eq!(from_buttons[5], 128);

        let from_axes = state(&[0.0, 0.0, 0.0, 0.0, 0.25, 0.75], &[]).report();
        assert_eq!(from_axes[4], 64);
        assert_eq!(from_axes[5], 191);
    }

    #[test]
    fn maps_w3c_buttons_to_xbox_hid_layout() {
        let mut buttons = vec![0.0; MAX_BUTTONS];
        for index in [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 16] {
            buttons[index] = 1.0;
        }
        let report = state(&[], &buttons).report();
        // A/B/X/Y + LB/RB -> 0b0011_1111; Back/Start -> 0b1100_0000.
        assert_eq!(report[1], 0xFF);
        // LS/RS -> bits 0/1; Guide -> bit 2.
        assert_eq!(report[2], 0b0000_0111);
        // Triggers are separate fields, never button bits.
        assert_eq!(&report[1..3], &[0xFF, 0x07]);
    }

    #[test]
    fn shoulders_sticks_menu_and_guide_keep_their_labels() {
        let mut buttons = vec![0.0; MAX_BUTTONS];
        // LB, RB, Back, Start, LS, RS, Guide.
        for index in [4, 5, 8, 9, 10, 11, 16] {
            buttons[index] = 1.0;
        }
        let report = state(&[], &buttons).report();
        assert_eq!(report[1] & 0b0011_0000, 0b0011_0000, "LB/RB bits 4/5");
        assert_eq!(report[1] & 0b1100_0000, 0b1100_0000, "Back/Start bits 6/7");
        assert_eq!(report[2] & 0b0000_0011, 0b0000_0011, "LS/RS bits 8/9");
        assert_eq!(report[2] & 0b0000_0100, 0b0000_0100, "Guide bit 10");
    }

    #[test]
    fn maps_dpad_to_hat_only() {
        let mut buttons = vec![0.0; MAX_BUTTONS];
        buttons[12] = 1.0; // up
        buttons[15] = 1.0; // right
        let report = state(&[], &buttons).report();
        assert_eq!(report[3], 1);
        assert_eq!(&report[1..3], &[0, 0]);
    }

    #[test]
    fn clamps_and_truncates_untrusted_input() {
        let mut axes = vec![f64::NAN, f64::INFINITY, -9.0, 9.0];
        axes.extend(std::iter::repeat(0.5).take(20));
        let mut buttons = vec![2.0, -1.0, f64::NAN];
        buttons.extend(std::iter::repeat(1.0).take(30));
        let normalized = state(&axes, &buttons).normalized();
        assert_eq!(normalized.axes.len(), MAX_AXES);
        assert_eq!(normalized.buttons.len(), MAX_BUTTONS);
        assert_eq!(normalized.axes[0], 0.0);
        assert_eq!(normalized.axes[1], 0.0);
        assert_eq!(normalized.axes[2], -1.0);
        assert_eq!(normalized.axes[3], 1.0);
        assert_eq!(normalized.buttons[0], 1.0);
        assert_eq!(normalized.buttons[1], 0.0);
        assert_eq!(normalized.buttons[2], 0.0);
    }

    #[test]
    fn report_descriptor_is_well_formed() {
        assert_eq!(REPORT_DESCRIPTOR[0], 0x05);
        assert_eq!(*REPORT_DESCRIPTOR.last().unwrap(), 0xC0);
        assert_eq!(REPORT_DESCRIPTOR.len(), 91);
    }
}

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
/// Number of digital buttons carried on the wire.
pub const MAX_BUTTONS: usize = 16;
/// Report ID + 6 axes + hat/padding + 16 buttons.
pub const REPORT_LEN: usize = 10;

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

    fn button_bits(&self) -> u16 {
        let mut bits = 0u16;
        for index in 0..MAX_BUTTONS {
            if self.pressed(index) {
                bits |= 1 << index;
            }
        }
        bits
    }

    /// Encodes the snapshot as the 10-byte input report described by
    /// [`REPORT_DESCRIPTOR`].
    pub fn report(&self) -> [u8; REPORT_LEN] {
        let mut report = [0u8; REPORT_LEN];
        report[0] = 0x01;
        report[1] = axis_byte(self.axis(0));
        report[2] = axis_byte(self.axis(1));
        report[3] = axis_byte(self.axis(2));
        report[4] = axis_byte(self.axis(3));
        report[5] = trigger_byte(self.trigger(6, 4));
        report[6] = trigger_byte(self.trigger(7, 5));
        report[7] = self.hat();
        let bits = self.button_bits();
        report[8] = (bits & 0xFF) as u8;
        report[9] = (bits >> 8) as u8;
        report
    }
}

fn axis_byte(value: f64) -> u8 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8
}

fn trigger_byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// HID report descriptor: a 6-axis, 16-button gamepad with a hat switch.
pub const REPORT_DESCRIPTOR: [u8; 75] = [
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x05, // Usage (Game Pad)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x01, //   Report ID (1)
    0x09, 0x30, //   Usage (X)
    0x09, 0x31, //   Usage (Y)
    0x09, 0x32, //   Usage (Z)
    0x09, 0x33, //   Usage (Rx)
    0x09, 0x34, //   Usage (Ry)
    0x09, 0x35, //   Usage (Rz)
    0x15, 0x00, //   Logical Minimum (0)
    0x26, 0xFF, 0x00, //   Logical Maximum (255)
    0x75, 0x08, //   Report Size (8)
    0x95, 0x06, //   Report Count (6)
    0x81, 0x02, //   Input (Data,Var,Abs)
    0x09, 0x39, //   Usage (Hat switch)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x07, //   Logical Maximum (7)
    0x35, 0x00, //   Physical Minimum (0)
    0x46, 0x3B, 0x01, //   Physical Maximum (315)
    0x65, 0x14, //   Unit (Eng Rot: Degree)
    0x75, 0x04, //   Report Size (4)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x42, //   Input (Data,Var,Abs,Null State)
    0x65, 0x00, //   Unit (None)
    0x05, 0x09, //   Usage Page (Button)
    0x19, 0x01, //   Usage Minimum (1)
    0x29, 0x10, //   Usage Maximum (16)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x10, //   Report Count (16)
    0x81, 0x02, //   Input (Data,Var,Abs)
    0x75, 0x04, //   Report Size (4)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x03, //   Input (Const,Var,Abs)
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
        properties.add(
            &CFString::from("ReportDescriptor"),
            &descriptor.as_CFType(),
        );
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
            &CFString::from("Virtual").as_CFType(),
        );
        properties.add(
            &CFString::from("VendorID"),
            &CFNumber::from(0x1234i32).as_CFType(),
        );
        properties.add(
            &CFString::from("ProductID"),
            &CFNumber::from(0x5678i32 + i32::from(slot)).as_CFType(),
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
            &CFNumber::from(0x05i32).as_CFType(),
        );

        let raw = unsafe {
            IOHIDUserDeviceCreate(
                std::ptr::null(),
                properties.as_concrete_TypeRef(),
            )
        };
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
        assert_eq!(&report[1..5], &[128, 128, 128, 128]);
        assert_eq!(&report[5..7], &[0, 0]);
        assert_eq!(report[7], 0x0F);
        assert_eq!(&report[8..10], &[0, 0]);
    }

    #[test]
    fn maps_stick_extremes_and_inverts_nothing() {
        let report = state(&[-1.0, 1.0, -1.0, 1.0], &[]).report();
        assert_eq!(&report[1..5], &[0, 255, 0, 255]);
    }

    #[test]
    fn maps_triggers_from_buttons_and_axes() {
        let from_buttons = state(&[], &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.5]).report();
        assert_eq!(from_buttons[5], 255);
        assert_eq!(from_buttons[6], 128);

        let from_axes = state(&[0.0, 0.0, 0.0, 0.0, 0.25, 0.75], &[]).report();
        assert_eq!(from_axes[5], 64);
        assert_eq!(from_axes[6], 191);
    }

    #[test]
    fn maps_dpad_to_hat_and_buttons() {
        let mut buttons = vec![0.0; 16];
        buttons[12] = 1.0; // up
        buttons[15] = 1.0; // right
        let report = state(&[], &buttons).report();
        assert_eq!(report[7], 1);
        assert_eq!(report[8], 0b0000_0000);
        assert_eq!(report[9], 0b1001_0000);
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
        assert_eq!(REPORT_DESCRIPTOR.len(), 75);
    }
}

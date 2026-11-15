use engine::{native_engine_start, native_engine_status, native_engine_stop, NativeEngine};
use input::{
    native_input_arm, native_input_button, native_input_pointer, native_input_scroll,
    native_input_status, native_input_text, NativeInput,
};
use serde::Serialize;
use std::process::Command;
use tauri::Manager;

mod adaptive;
mod capture;
#[cfg(target_os = "macos")]
mod client;
mod clipboard;
mod congestion;
#[cfg(target_os = "macos")]
mod display_info;
mod engine;
mod gamepad;
#[cfg(target_os = "macos")]
mod host;
mod input;
mod join_code;
mod network;
mod ojd;
#[cfg(target_os = "macos")]
mod preview;
mod public_address;
#[cfg(target_os = "macos")]
mod reachability;
mod rtp_video;
mod transport;
#[cfg(target_os = "macos")]
mod video;
mod webrtc_transport;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    hostname: String,
    os: String,
    os_version: String,
    arch: String,
    model: String,
    chip: String,
    app_version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionStatus {
    screen_recording: bool,
    accessibility: bool,
    input_monitoring: bool,
    platform_supported: bool,
}

fn output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_string())
        .unwrap_or_default()
}

fn arch_label() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

#[tauri::command]
fn system_info(app: tauri::AppHandle) -> SystemInfo {
    #[cfg(target_os = "macos")]
    {
        let hostname = {
            let value = output("hostname", &[]);
            if value.is_empty() {
                "This Mac".to_string()
            } else {
                value
            }
        };
        let chip = {
            let value = output("sysctl", &["-n", "machdep.cpu.brand_string"]);
            if value.is_empty() {
                "Apple Silicon".to_string()
            } else {
                value
            }
        };
        SystemInfo {
            hostname,
            os: "macOS".to_string(),
            os_version: output("sw_vers", &["-productVersion"]),
            arch: arch_label(),
            model: output("sysctl", &["-n", "hw.model"]),
            chip,
            app_version: app.package_info().version.to_string(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "This computer".to_string());
        SystemInfo {
            hostname,
            os: std::env::consts::OS.to_string(),
            os_version: String::new(),
            arch: arch_label(),
            model: String::new(),
            chip: String::new(),
            app_version: app.package_info().version.to_string(),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod mac_permissions {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub fn CGPreflightScreenCaptureAccess() -> u8;
        pub fn CGRequestScreenCaptureAccess() -> u8;
        pub fn CGPreflightListenEventAccess() -> u8;
        pub fn CGRequestListenEventAccess() -> u8;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXIsProcessTrusted() -> u8;
    }
}

#[tauri::command]
fn permission_status() -> PermissionStatus {
    #[cfg(target_os = "macos")]
    unsafe {
        PermissionStatus {
            screen_recording: mac_permissions::CGPreflightScreenCaptureAccess() != 0,
            accessibility: mac_permissions::AXIsProcessTrusted() != 0,
            input_monitoring: mac_permissions::CGPreflightListenEventAccess() != 0,
            platform_supported: true,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionStatus {
            screen_recording: false,
            accessibility: false,
            input_monitoring: false,
            platform_supported: false,
        }
    }
}

#[tauri::command]
fn request_permission(kind: String) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        match kind.as_str() {
            "screen" => Ok(unsafe { mac_permissions::CGRequestScreenCaptureAccess() != 0 }),
            "input" => Ok(unsafe { mac_permissions::CGRequestListenEventAccess() != 0 }),
            "accessibility" => {
                open_privacy_settings("accessibility".to_string())?;
                Ok(unsafe { mac_permissions::AXIsProcessTrusted() != 0 })
            }
            other => Err(format!("Unknown permission: {other}")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        Err("Native permission requests are only supported on macOS".to_string())
    }
}

#[tauri::command]
fn open_privacy_settings(kind: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let anchor = match kind.as_str() {
            "screen" => "Privacy_ScreenCapture",
            "accessibility" => "Privacy_Accessibility",
            "input" => "Privacy_ListenEvent",
            "microphone" => "Privacy_Microphone",
            other => return Err(format!("Unknown privacy pane: {other}")),
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|error| format!("Could not open System Settings: {error}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        Err("Privacy settings are only supported on macOS".to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAddressInfo {
    address: String,
    port: u16,
    local_address: String,
}

/// Asks a STUN server which public address:port it sees for a UDP socket.
///
/// This is the address a host shares so a peer on another network can aim at
/// it, and probing from the media port doubles as a router port-forward check:
/// if the reported port is not the forwarded one, the rule is wrong.
#[tauri::command(async)]
fn native_discover_public_address(
    stun_url: Option<String>,
    local_port: Option<u16>,
) -> Result<PublicAddressInfo, String> {
    let stun_url = stun_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Enter a STUN server, for example stun:stun.l.google.com:19302.".to_string()
        })?;
    let found = public_address::discover(
        stun_url,
        local_port.unwrap_or(0),
        std::time::Duration::from_secs(4),
    )?;
    Ok(PublicAddressInfo {
        address: found.address.ip().to_string(),
        port: found.address.port(),
        local_address: found.local_address.to_string(),
    })
}

#[tauri::command]
fn generate_pair_code() -> String {
    transport::generate_pair_code()
}

/// Lists the addresses a friend could dial, with the Tailscale tailnet marked so
/// the Host screen can default to the overlay address.
#[tauri::command(async)]
fn native_network_addresses() -> network::NetworkInfo {
    network::network_info()
}

/// Backing pixel size of the main display, used by the "match display" capture
/// preset so a 16:10 panel is not forced into a 16:9 frame.
#[cfg(target_os = "macos")]
#[derive(serde::Serialize)]
struct DisplaySize {
    width: u32,
    height: u32,
}

#[cfg(target_os = "macos")]
#[tauri::command(async)]
fn native_display_size() -> Result<DisplaySize, String> {
    let (width, height) = display_info::main_display_size()?;
    Ok(DisplaySize { width, height })
}

/// Lists every shareable display for the Host screen's multi-monitor picker.
#[cfg(target_os = "macos")]
#[tauri::command(async)]
fn native_list_displays() -> Result<Vec<display_info::DisplayInfo>, String> {
    display_info::displays()
}

/// Packs a host address, TCP port and the current pairing code into one
/// shareable join code. The code is a single-use secret.
#[tauri::command]
fn native_make_join_code(address: String, port: u16, code: String) -> Result<String, String> {
    join_code::encode(&address, port, &code)
}

/// Validates and unpacks a join code so the client can connect with the values
/// it carries.
#[tauri::command]
fn native_parse_join_code(join_code: String) -> Result<join_code::JoinCode, String> {
    join_code::decode(&join_code)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(NativeEngine::default())
        .manage(NativeInput::default());

    #[cfg(target_os = "macos")]
    let builder = builder
        .manage(host::NativeHost::default())
        .manage(client::NativeClient::default())
        .manage(video::VideoSurface::default())
        .invoke_handler(tauri::generate_handler![
            system_info,
            permission_status,
            request_permission,
            open_privacy_settings,
            generate_pair_code,
            native_discover_public_address,
            native_network_addresses,
            native_make_join_code,
            native_parse_join_code,
            native_display_size,
            native_list_displays,
            native_engine_start,
            native_engine_stop,
            native_engine_status,
            native_input_arm,
            native_input_status,
            native_input_pointer,
            native_input_button,
            native_input_scroll,
            native_input_text,
            host::native_host_start,
            host::native_host_stop,
            host::native_host_status,
            host::native_host_rotate_code,
            host::native_host_set_input,
            host::native_host_set_gamepad,
            host::native_host_set_approval,
            host::native_host_respond,
            host::native_host_set_peer_permissions,
            host::native_host_kick,
            client::native_client_connect,
            client::native_client_disconnect,
            client::native_client_status,
            client::native_client_input,
            client::native_client_request_keyframe,
            client::native_client_request_display,
            video::native_video_attach,
            video::native_video_bounds,
            video::native_video_detach,
            video::native_video_size,
            reachability::native_discover_hosts
        ]);

    #[cfg(not(target_os = "macos"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        system_info,
        permission_status,
        request_permission,
        open_privacy_settings,
        generate_pair_code,
        native_discover_public_address,
        native_network_addresses,
        native_make_join_code,
        native_parse_join_code,
        native_engine_start,
        native_engine_stop,
        native_engine_status,
        native_input_arm,
        native_input_status,
        native_input_pointer,
        native_input_button,
        native_input_scroll,
        native_input_text
    ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while running ParsecPlus");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            #[cfg(target_os = "macos")]
            {
                handle.state::<host::NativeHost>().shutdown();
                handle.state::<client::NativeClient>().shutdown();
            }
            handle.state::<NativeEngine>().shutdown();
            handle.state::<NativeInput>().shutdown();
        }
    });
}

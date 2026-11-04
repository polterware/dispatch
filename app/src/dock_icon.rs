//! The app icon in the Dock for a build run from `cargo run`, which has no bundle and
//! so no `Assets.car`: the same icon the bundle and the Electron app show. A bundled
//! app keeps its Icon Composer icon.

/// The Icon Composer export of `resources/dispatch.icon` (`dispatch Exports`, Default) on the macOS
/// icon grid.
const ICON_PNG: &[u8] = include_bytes!("../resources/dispatch-icon.png");

#[cfg(target_os = "macos")]
pub fn install() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let bundled = std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
    if bundled {
        return;
    }
    unsafe {
        let data: *mut Object = msg_send![class!(NSData),
            dataWithBytes: ICON_PNG.as_ptr()
            length: ICON_PNG.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![app, setApplicationIconImage: image];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}

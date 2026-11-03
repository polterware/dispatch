//! Sparkle, as the Electron app's `updater.mm` drives it: one `SPUStandardUpdaterController`
//! for the life of the process, checking daily against the feed the bundle's `SUFeedURL`
//! names, and Check for Updates… in the app menu.
//!
//! Sparkle.framework ships in the bundle's `Contents/Frameworks` (`tooling/release`). A
//! build run from `cargo run` has no bundle, so the updater stays off.

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;

    use objc::runtime::{BOOL, NO, Object, YES};
    use objc::{class, msg_send, sel, sel_impl};

    thread_local! {
        static CONTROLLER: Cell<*mut Object> = const { Cell::new(std::ptr::null_mut()) };
    }

    const CHECK_INTERVAL_SECONDS: f64 = 86_400.0;

    fn ns_string(text: &str) -> *mut Object {
        let bytes = std::ffi::CString::new(text).unwrap_or_default();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: bytes.as_ptr()] }
    }

    /// Loads Sparkle from the bundle and starts its updater. Call once on the main thread.
    pub fn start() -> Result<(), String> {
        if !CONTROLLER.with(Cell::get).is_null() {
            return Ok(());
        }
        unsafe {
            let main: *mut Object = msg_send![class!(NSBundle), mainBundle];
            let frameworks: *mut Object = msg_send![main, privateFrameworksPath];
            if frameworks.is_null() {
                return Err("no bundle".into());
            }
            let path: *mut Object = msg_send![frameworks, stringByAppendingPathComponent: ns_string("Sparkle.framework")];
            let bundle: *mut Object = msg_send![class!(NSBundle), bundleWithPath: path];
            if bundle.is_null() {
                return Err("Sparkle.framework is not in the bundle".into());
            }
            let loaded: BOOL = msg_send![bundle, load];
            if loaded == NO {
                return Err("Sparkle.framework did not load".into());
            }
            let Some(class) = objc::runtime::Class::get("SPUStandardUpdaterController") else {
                return Err("SPUStandardUpdaterController is missing".into());
            };
            let controller: *mut Object = msg_send![class, alloc];
            let nil: *mut Object = std::ptr::null_mut();
            let controller: *mut Object = msg_send![controller,
                initWithStartingUpdater: NO
                updaterDelegate: nil
                userDriverDelegate: nil];
            if controller.is_null() {
                return Err("Sparkle could not start".into());
            }
            let updater: *mut Object = msg_send![controller, updater];
            let _: () = msg_send![updater, setUpdateCheckInterval: CHECK_INTERVAL_SECONDS];
            let mut error: *mut Object = std::ptr::null_mut();
            let started: BOOL = msg_send![updater, startUpdater: &mut error];
            if started != YES {
                return Err("Sparkle could not start its updater".into());
            }
            // Kept for the life of the process, as Sparkle's programmatic setup requires.
            CONTROLLER.with(|cell| cell.set(controller));
        }
        Ok(())
    }

    /// Checks for an update now, with Sparkle's own window.
    pub fn check() {
        let controller = CONTROLLER.with(Cell::get);
        if controller.is_null() {
            return;
        }
        unsafe {
            let updater: *mut Object = msg_send![controller, updater];
            let can: BOOL = msg_send![updater, canCheckForUpdates];
            if can == YES {
                let nil: *mut Object = std::ptr::null_mut();
                let _: () = msg_send![controller, checkForUpdates: nil];
            }
        }
    }

    /// Whether Sparkle runs in this process (a bundled build).
    pub fn available() -> bool {
        !CONTROLLER.with(Cell::get).is_null()
    }
}

#[cfg(target_os = "macos")]
pub use mac::{available, check, start};

#[cfg(not(target_os = "macos"))]
pub fn start() -> Result<(), String> {
    Err("updates are macOS only".into())
}

#[cfg(not(target_os = "macos"))]
pub fn check() {}

#[cfg(not(target_os = "macos"))]
pub fn available() -> bool {
    false
}

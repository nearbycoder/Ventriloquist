//! Updates, through Sparkle.
//!
//! A bundle built for release (`scripts/bundle --updates`) carries Sparkle.framework and, in
//! its Info.plist, the feed to read and the key each update is signed with. Sparkle does the
//! rest: it checks the feed once a day, shows what's new, verifies the download, swaps the
//! bundle and relaunches. A bundle without the framework, such as a development build, has no
//! updater and no menu item.

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};

use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
}
const RTLD_NOW: c_int = 0x2;
const RTLD_GLOBAL: c_int = 0x8;

/// Sparkle's controller, kept for the life of the process.
static CONTROLLER: AtomicPtr<AnyObject> = AtomicPtr::new(null_mut());

fn framework() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let library = exe.parent()?.parent()?.join("Frameworks/Sparkle.framework/Sparkle");
    library.is_file().then_some(library)
}

/// Whether this build can update itself.
pub fn available() -> bool {
    framework().is_some()
}

/// Starts the updater and its daily check. Call once, on the main thread.
pub fn start() {
    if !CONTROLLER.load(Ordering::Acquire).is_null() {
        return;
    }
    let Some(path) = framework().and_then(|library| CString::new(library.as_os_str().as_bytes()).ok()) else { return };
    // SAFETY: the path is a NUL-terminated string that outlives the call.
    if unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) }.is_null() {
        return;
    }
    let Some(class) = AnyClass::get("SPUStandardUpdaterController") else { return };
    // SAFETY: the selectors and argument types are Sparkle's; both delegates may be nil.
    let controller: *mut AnyObject = unsafe {
        let controller: *mut AnyObject = msg_send![class, alloc];
        msg_send![controller, initWithStartingUpdater: true, updaterDelegate: null::<AnyObject>(), userDriverDelegate: null::<AnyObject>()]
    };
    CONTROLLER.store(controller, Ordering::Release);
}

/// Checks now and says what it found, as the menu item does. Main thread.
pub fn check() {
    let controller = CONTROLLER.load(Ordering::Acquire);
    if !controller.is_null() {
        // SAFETY: the controller is alive (never released) and `checkForUpdates:` takes any sender.
        let _: () = unsafe { msg_send![controller, checkForUpdates: null::<AnyObject>()] };
    }
}

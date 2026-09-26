//! Launch at login via `SMAppService.mainAppService` (macOS 13+).
//!
//! Only works when running from a `.app` bundle (ideally signed; ad-hoc works
//! for local builds). From a bare `cargo run` binary every call returns a clean
//! `Err(SmartError::Platform(..))` / `LoginItemStatus::Unavailable`.
//!
//! Calls are fast IPC (< 50 ms) and thread-safe; calling from the main thread
//! in response to a preference toggle is fine. After `set_launch_at_login(true)`
//! the status may be [`LoginItemStatus::RequiresApproval`] — offer
//! [`open_login_items_settings`] in that case.

use crate::Result;

/// Registration state of the main-app login item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    /// Not registered.
    NotRegistered,
    /// Registered and allowed to launch at login.
    Enabled,
    /// Registered, but the user must approve it in System Settings → Login Items.
    RequiresApproval,
    /// The service could not find the app (e.g. moved/translocated bundle).
    NotFound,
    /// Not supported here (not a bundled `.app`, or macOS < 13 / not macOS).
    Unavailable,
}

/// Register / unregister the app as a login item. Idempotent.
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    imp::set(enabled)
}

/// Current registration state (`SMAppService.status == .enabled`).
pub fn is_launch_at_login_enabled() -> bool {
    login_item_status() == LoginItemStatus::Enabled
}

/// Detailed registration state.
pub fn login_item_status() -> LoginItemStatus {
    imp::status()
}

/// Open System Settings → General → Login Items.
pub fn open_login_items_settings() -> Result<()> {
    imp::open_settings()
}

/// True when the current process runs from a `.app` bundle with a bundle id.
pub fn is_running_from_app_bundle() -> bool {
    imp::is_bundled()
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use objc2_foundation::{NSBundle, NSError, NSInteger};

    use super::LoginItemStatus;
    use crate::{Result, SmartError};

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}

    pub(super) fn is_bundled() -> bool {
        let b = NSBundle::mainBundle();
        let path = b.bundlePath().to_string();
        path.ends_with(".app") && b.bundleIdentifier().is_some()
    }

    fn service() -> Result<Retained<AnyObject>> {
        if !is_bundled() {
            return Err(SmartError::Platform(
                "launch at login requires running from the RapidSnap.app bundle".into(),
            ));
        }
        let cls = AnyClass::get(c"SMAppService")
            .ok_or_else(|| SmartError::Platform("SMAppService unavailable (requires macOS 13+)".into()))?;
        // SAFETY: `+[SMAppService mainAppService]` returns a non-null autoreleased object.
        let svc: Option<Retained<AnyObject>> = unsafe { msg_send![cls, mainAppService] };
        svc.ok_or_else(|| SmartError::Platform("SMAppService.mainAppService returned nil".into()))
    }

    fn raw_status(svc: &AnyObject) -> LoginItemStatus {
        // SAFETY: `status` is an NSInteger-backed enum property.
        let s: NSInteger = unsafe { msg_send![svc, status] };
        match s {
            0 => LoginItemStatus::NotRegistered,
            1 => LoginItemStatus::Enabled,
            2 => LoginItemStatus::RequiresApproval,
            3 => LoginItemStatus::NotFound,
            _ => LoginItemStatus::Unavailable,
        }
    }

    pub(super) fn status() -> LoginItemStatus {
        match service() {
            Ok(svc) => raw_status(&svc),
            Err(_) => LoginItemStatus::Unavailable,
        }
    }

    fn err(e: Retained<NSError>, what: &str) -> SmartError {
        SmartError::Platform(format!("{what}: {} (code {})", e.localizedDescription(), e.code()))
    }

    pub(super) fn set(enabled: bool) -> Result<()> {
        let svc = service()?;
        let st = raw_status(&svc);
        if enabled {
            if matches!(st, LoginItemStatus::Enabled | LoginItemStatus::RequiresApproval) {
                return Ok(());
            }
            // SAFETY: `-registerAndReturnError:` → BOOL + NSError**.
            let r: std::result::Result<(), Retained<NSError>> = unsafe { msg_send![&svc, registerAndReturnError: _] };
            r.map_err(|e| err(e, "SMAppService register"))
        } else {
            if matches!(st, LoginItemStatus::NotRegistered | LoginItemStatus::NotFound) {
                return Ok(());
            }
            // SAFETY: `-unregisterAndReturnError:` → BOOL + NSError**.
            let r: std::result::Result<(), Retained<NSError>> =
                unsafe { msg_send![&svc, unregisterAndReturnError: _] };
            r.map_err(|e| err(e, "SMAppService unregister"))
        }
    }

    pub(super) fn open_settings() -> Result<()> {
        if let Some(cls) = AnyClass::get(c"SMAppService") {
            // SAFETY: class method with no arguments / no return value.
            let _: () = unsafe { msg_send![cls, openSystemSettingsLoginItems] };
            return Ok(());
        }
        std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.LoginItems-Settings.extension")
            .status()
            .map_err(SmartError::Io)
            .and_then(|s| if s.success() { Ok(()) } else { Err(SmartError::Platform(format!("open exited {s}"))) })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::LoginItemStatus;
    use crate::{Result, SmartError};

    pub(super) fn is_bundled() -> bool {
        false
    }
    pub(super) fn status() -> LoginItemStatus {
        LoginItemStatus::Unavailable
    }
    pub(super) fn set(_enabled: bool) -> Result<()> {
        Err(SmartError::Platform("launch at login is only implemented on macOS".into()))
    }
    pub(super) fn open_settings() -> Result<()> {
        Err(SmartError::Platform("not supported on this platform".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbundled_test_binary_is_clean_error() {
        // `cargo test` binaries are never inside a .app bundle.
        assert!(!is_running_from_app_bundle());
        assert_eq!(login_item_status(), LoginItemStatus::Unavailable);
        assert!(!is_launch_at_login_enabled());
        assert!(matches!(set_launch_at_login(true), Err(crate::SmartError::Platform(_))));
    }
}

//! How hot the device is, from the OS: `NSProcessInfo.thermalState` on iOS
//! and macOS, the NDK thermal API on Android 11+ (looked up at run time, so
//! older phones just report nothing). Elsewhere: unknown.

pub use wp_render::governor::Thermal;

/// Reads the thermal state; keep one and call [`ThermalProbe::read`] every
/// second or two.
pub struct ThermalProbe {
    #[cfg(target_os = "android")]
    android: Option<android::Manager>,
}

impl ThermalProbe {
    pub fn new() -> Self {
        ThermalProbe {
            #[cfg(target_os = "android")]
            android: android::Manager::acquire(),
        }
    }

    /// The current state, if the platform reports one.
    pub fn read(&self) -> Option<Thermal> {
        #[cfg(target_os = "android")]
        return self.android.as_ref().map(android::Manager::status);
        #[cfg(target_vendor = "apple")]
        return apple::state();
        #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
        None
    }
}

impl Default for ThermalProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_vendor = "apple")]
mod apple {
    use super::Thermal;
    use std::ffi::{c_char, c_void};

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    /// `[[NSProcessInfo processInfo] thermalState]`: 0 nominal … 3 critical.
    pub fn state() -> Option<Thermal> {
        // SAFETY: objc_msgSend is called through the exact signatures of the
        // two methods used (`+processInfo` returns an object, `-thermalState`
        // an NSInteger); the class and selector names are NUL-terminated.
        unsafe {
            let class = objc_getClass(c"NSProcessInfo".as_ptr());
            if class.is_null() {
                return None;
            }
            let send_obj: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_int: extern "C" fn(*mut c_void, *mut c_void) -> isize =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let info = send_obj(class, sel_registerName(c"processInfo".as_ptr()));
            if info.is_null() {
                return None;
            }
            Some(
                match send_int(info, sel_registerName(c"thermalState".as_ptr())) {
                    0 => Thermal::Nominal,
                    1 => Thermal::Fair,
                    2 => Thermal::Serious,
                    _ => Thermal::Critical,
                },
            )
        }
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::Thermal;
    use std::ffi::c_void;

    type Acquire = unsafe extern "C" fn() -> *mut c_void;
    type Status = unsafe extern "C" fn(*mut c_void) -> i32;

    /// An `AThermalManager` and the function that reads it (API 30+).
    pub struct Manager {
        manager: *mut c_void,
        status: Status,
    }

    // SAFETY: AThermalManager may be used from any thread (NDK docs).
    unsafe impl Send for Manager {}

    impl Manager {
        pub fn acquire() -> Option<Manager> {
            // SAFETY: dlopen/dlsym with NUL-terminated names; the symbols, when
            // present, have the NDK signatures declared above.
            unsafe {
                let lib = libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW);
                if lib.is_null() {
                    return None;
                }
                let acquire = libc::dlsym(lib, c"AThermal_acquireManager".as_ptr());
                let status = libc::dlsym(lib, c"AThermal_getCurrentThermalStatus".as_ptr());
                if acquire.is_null() || status.is_null() {
                    return None;
                }
                let acquire: Acquire = std::mem::transmute(acquire);
                let manager = acquire();
                (!manager.is_null()).then(|| Manager {
                    manager,
                    status: std::mem::transmute::<*mut c_void, Status>(status),
                })
            }
        }

        /// ATHERMAL_STATUS_*: 0 none, 1 light, 2 moderate, 3 severe,
        /// 4 critical, 5 emergency, 6 shutdown (negative: error).
        pub fn status(&self) -> Thermal {
            // SAFETY: `manager` came from AThermal_acquireManager and is kept alive.
            match unsafe { (self.status)(self.manager) } {
                ..=0 => Thermal::Nominal,
                1 | 2 => Thermal::Fair,
                3 => Thermal::Serious,
                _ => Thermal::Critical,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_mac_reports_a_state() {
        let t = super::ThermalProbe::new().read();
        if cfg!(target_os = "macos") {
            assert!(t.is_some());
        }
    }
}

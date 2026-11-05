//! Which way is up, from the device's motion sensors, for tilt steering
//! (`wp_vehicle::tilt`).
//!
//! Android: the NDK sensor API on a thread of its own (the gravity sensor,
//! else the accelerometer), so the app's event loop is never involved.
//! iOS: CoreMotion's gravity (fused, so hand shake and jolts are filtered).
//! Desktops have no sensor; [`MotionSensor::fake`] stands in for testing.
//!
//! Readings are "up" in the device frame: x to the right and y to the top of
//! the screen in its natural (portrait) orientation, z out of the screen.

use wp_core::glam::Vec3;

/// A running motion sensor. Dropping it stops the sensor (keep it only
/// while tilt steering is on, to save battery).
pub struct MotionSensor {
    source: Source,
}

enum Source {
    /// Straight until `turn_at`, then turned.
    Fake {
        turn_at: std::time::Instant,
        turned: Vec3,
    },
    #[cfg(target_os = "android")]
    Android(android::Reader),
    #[cfg(target_os = "ios")]
    Ios(ios::Manager),
}

impl MotionSensor {
    /// The device's sensor, if it has one.
    pub fn open() -> Option<MotionSensor> {
        #[cfg(target_os = "android")]
        return android::Reader::start().map(|r| MotionSensor {
            source: Source::Android(r),
        });
        #[cfg(target_os = "ios")]
        return ios::Manager::start().map(|m| MotionSensor {
            source: Source::Ios(m),
        });
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        None
    }

    /// A pretend sensor (testing on desktops): a phone held in landscape,
    /// leaning back 40°, straight for `straight_s` (so that is straight
    /// ahead), then turned `turn_deg` anticlockwise.
    pub fn fake(turn_deg: f32, straight_s: f32) -> MotionSensor {
        MotionSensor {
            source: Source::Fake {
                turn_at: std::time::Instant::now()
                    + std::time::Duration::from_secs_f32(straight_s.max(0.0)),
                turned: wp_vehicle::tilt::held_landscape(turn_deg, 40.0),
            },
        }
    }

    /// The latest "up" in the device frame, once the sensor has reported.
    pub fn up(&self) -> Option<Vec3> {
        match &self.source {
            Source::Fake { turn_at, turned } => Some(if std::time::Instant::now() < *turn_at {
                wp_vehicle::tilt::held_landscape(0.0, 40.0)
            } else {
                *turned
            }),
            #[cfg(target_os = "android")]
            Source::Android(r) => r.latest(),
            #[cfg(target_os = "ios")]
            Source::Ios(m) => m.gravity().map(|g| -g),
        }
    }
}

#[cfg(target_os = "android")]
mod android {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use wp_core::glam::Vec3;

    const GRAVITY: i32 = 9;
    const ACCELEROMETER: i32 = 1;
    const ALLOW_NON_CALLBACKS: i32 = 1;

    /// `ASensorEvent` (104 bytes): header, then 16 floats of data.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Event {
        version: i32,
        sensor: i32,
        kind: i32,
        reserved0: i32,
        timestamp: i64,
        data: [f32; 16],
        flags: u32,
        reserved1: [i32; 3],
    }

    #[link(name = "android")]
    unsafe extern "C" {
        fn ASensorManager_getInstance() -> *mut c_void;
        fn ASensorManager_getDefaultSensor(manager: *mut c_void, kind: i32) -> *const c_void;
        fn ASensorManager_createEventQueue(
            manager: *mut c_void,
            looper: *mut c_void,
            ident: i32,
            callback: *const c_void,
            data: *mut c_void,
        ) -> *mut c_void;
        fn ASensorManager_destroyEventQueue(manager: *mut c_void, queue: *mut c_void) -> i32;
        fn ASensorEventQueue_enableSensor(queue: *mut c_void, sensor: *const c_void) -> i32;
        fn ASensorEventQueue_disableSensor(queue: *mut c_void, sensor: *const c_void) -> i32;
        fn ASensorEventQueue_setEventRate(
            queue: *mut c_void,
            sensor: *const c_void,
            usec: i32,
        ) -> i32;
        fn ASensorEventQueue_getEvents(
            queue: *mut c_void,
            events: *mut Event,
            count: usize,
        ) -> isize;
        fn ALooper_prepare(opts: i32) -> *mut c_void;
        fn ALooper_pollOnce(
            timeout_ms: i32,
            fd: *mut i32,
            events: *mut i32,
            data: *mut *mut c_void,
        ) -> i32;
    }

    /// The sensor thread and its latest reading.
    pub struct Reader {
        latest: Arc<Mutex<Option<Vec3>>>,
        stop: Arc<AtomicBool>,
    }

    impl Reader {
        pub fn start() -> Option<Reader> {
            let latest = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));
            let (ok_tx, ok_rx) = std::sync::mpsc::channel();
            let (l, s) = (latest.clone(), stop.clone());
            std::thread::Builder::new()
                .name("waypoint-motion".into())
                .spawn(move || run(l, s, ok_tx))
                .ok()?;
            // The thread says whether the device has a sensor.
            ok_rx.recv().ok()?.then_some(Reader { latest, stop })
        }

        pub fn latest(&self) -> Option<Vec3> {
            *self.latest.lock().ok()?
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn run(
        latest: Arc<Mutex<Option<Vec3>>>,
        stop: Arc<AtomicBool>,
        ok: std::sync::mpsc::Sender<bool>,
    ) {
        // SAFETY: NDK calls with the documented signatures. The looper,
        // manager, sensor and queue are created on this thread and used only
        // here; `events` has room for the count passed.
        unsafe {
            let looper = ALooper_prepare(ALLOW_NON_CALLBACKS);
            let manager = ASensorManager_getInstance();
            if looper.is_null() || manager.is_null() {
                let _ = ok.send(false);
                return;
            }
            let mut sensor = ASensorManager_getDefaultSensor(manager, GRAVITY);
            if sensor.is_null() {
                sensor = ASensorManager_getDefaultSensor(manager, ACCELEROMETER);
            }
            if sensor.is_null() {
                let _ = ok.send(false);
                return;
            }
            let queue = ASensorManager_createEventQueue(
                manager,
                looper,
                1,
                std::ptr::null(),
                std::ptr::null_mut(),
            );
            if queue.is_null() {
                let _ = ok.send(false);
                return;
            }
            ASensorEventQueue_enableSensor(queue, sensor);
            ASensorEventQueue_setEventRate(queue, sensor, 16_667);
            let _ = ok.send(true);
            let mut events = [std::mem::zeroed::<Event>(); 16];
            while !stop.load(Ordering::Relaxed) {
                ALooper_pollOnce(
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                let mut newest = None;
                loop {
                    let n = ASensorEventQueue_getEvents(queue, events.as_mut_ptr(), events.len());
                    if n <= 0 {
                        break;
                    }
                    let e = &events[n as usize - 1];
                    // At rest both sensors report the push against gravity: up.
                    newest = Some(Vec3::new(e.data[0], e.data[1], e.data[2]));
                }
                if let (Some(u), Ok(mut l)) = (newest, latest.lock()) {
                    *l = Some(u);
                }
            }
            ASensorEventQueue_disableSensor(queue, sensor);
            ASensorManager_destroyEventQueue(manager, queue);
        }
    }
}

#[cfg(target_os = "ios")]
mod ios {
    use std::ffi::{c_char, c_void};
    use wp_core::glam::Vec3;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    #[link(name = "CoreMotion", kind = "framework")]
    unsafe extern "C" {}

    /// `CMAcceleration`: three doubles, in g.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Acceleration {
        x: f64,
        y: f64,
        z: f64,
    }

    type Obj = *mut c_void;

    fn sel(name: &std::ffi::CStr) -> Obj {
        // SAFETY: a NUL-terminated selector name.
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// Call `objc_msgSend` as a function of type `F`.
    ///
    /// # Safety
    /// `F` must match the method's real signature.
    unsafe fn msg<F: Copy>() -> F {
        let f = objc_msgSend as unsafe extern "C" fn();
        // SAFETY: function pointers are the same size; the caller vouches for `F`.
        unsafe { std::mem::transmute_copy(&f) }
    }

    /// A started `CMMotionManager`.
    pub struct Manager(Obj);

    impl Manager {
        pub fn start() -> Option<Manager> {
            // SAFETY: each call uses the real signature of the method it sends:
            // +alloc/-init/-deviceMotion return objects, -isDeviceMotionAvailable
            // a BOOL, -setDeviceMotionUpdateInterval: takes an NSTimeInterval.
            unsafe {
                let class = objc_getClass(c"CMMotionManager".as_ptr());
                if class.is_null() {
                    return None;
                }
                let obj: extern "C" fn(Obj, Obj) -> Obj = msg();
                let boolean: extern "C" fn(Obj, Obj) -> bool = msg();
                let set_f64: extern "C" fn(Obj, Obj, f64) = msg();
                let manager = obj(obj(class, sel(c"alloc")), sel(c"init"));
                if manager.is_null() {
                    return None;
                }
                let m = Manager(manager);
                if !boolean(manager, sel(c"isDeviceMotionAvailable")) {
                    return None;
                }
                set_f64(manager, sel(c"setDeviceMotionUpdateInterval:"), 1.0 / 60.0);
                obj(manager, sel(c"startDeviceMotionUpdates"));
                Some(m)
            }
        }

        /// Gravity in the device frame (pointing down), in g.
        pub fn gravity(&self) -> Option<Vec3> {
            // SAFETY: -deviceMotion returns a CMDeviceMotion (or nil) and
            // -gravity a CMAcceleration, which arm64 returns in registers, so
            // plain objc_msgSend is right (x86_64 would need the _stret form).
            #[cfg(target_arch = "aarch64")]
            unsafe {
                let obj: extern "C" fn(Obj, Obj) -> Obj = msg();
                let motion = obj(self.0, sel(c"deviceMotion"));
                if motion.is_null() {
                    return None;
                }
                let acc: extern "C" fn(Obj, Obj) -> Acceleration = msg();
                let g = acc(motion, sel(c"gravity"));
                Some(Vec3::new(g.x as f32, g.y as f32, g.z as f32))
            }
            #[cfg(not(target_arch = "aarch64"))]
            None
        }
    }

    impl Drop for Manager {
        fn drop(&mut self) {
            // SAFETY: both methods take no arguments and return nothing we use.
            unsafe {
                let send: extern "C" fn(Obj, Obj) -> Obj = msg();
                send(self.0, sel(c"stopDeviceMotionUpdates"));
                send(self.0, sel(c"release"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_sensor_steers() {
        let s = MotionSensor::fake(10.0, 0.0);
        let mut t = wp_vehicle::TiltSteering::default();
        // Straight ahead is how the phone was held at first...
        t.update(wp_vehicle::tilt::held_landscape(0.0, 40.0), 1.0 / 60.0);
        let mut out = 0.0;
        for _ in 0..60 {
            out = t.update(s.up().unwrap(), 1.0 / 60.0);
        }
        // ...so 10° anticlockwise steers left.
        assert!(out > 0.1, "{out}");
    }

    #[test]
    fn desktops_have_no_sensor() {
        if cfg!(not(any(target_os = "android", target_os = "ios"))) {
            assert!(MotionSensor::open().is_none());
        }
    }
}

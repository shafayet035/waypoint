//! The game player shared by every platform. Desktop and iOS use the
//! `waypoint-player` binary (`main.rs`); Android loads this crate as
//! `libwp_player.so` through `NativeActivity` and calls `android_main`.

use std::path::Path;
use waypoint::core::Diagnostic;
use waypoint::project::Project;
use wp_control::Session;
use wp_platform::{WindowOptions, WindowSummary};

/// Load the project at `root` and play it with `run` (the platform's window loop).
/// `data_dir` is the app's private storage (phones); `None` uses the user's
/// data folder. Device settings (the measured quality tier) live there.
pub fn play(
    root: &Path,
    data_dir: Option<&Path>,
    scenario: Option<&str>,
    exit_after_s: Option<f32>,
    run: impl FnOnce(Session, WindowOptions) -> Result<WindowSummary, Diagnostic>,
) -> Result<WindowSummary, Vec<Diagnostic>> {
    let (project, diags) = Project::load(root);
    let project = match project {
        Some(p) if !diags.has_errors() => p,
        _ => return Err(diags.0),
    };
    let quality = &project.game.value.quality;
    let tier = if quality.auto {
        let path = wp_platform::settings::settings_path(data_dir, &project.game.value.name);
        let (tier, how) = wp_platform::settings::device_tier(path.as_deref(), quality.tier);
        log(&how.describe(tier));
        tier
    } else {
        quality.tier
    };
    let (app, player) = waypoint::start_at(&project, scenario, 0, tier).map_err(|d| vec![d])?;
    let vehicles = project
        .vehicles
        .iter()
        .map(|(k, v)| (k.clone(), v.value.clone()))
        .collect();
    let session = Session::new(app, Some(player), vehicles);
    let control = match project.game.value.dev.control_port {
        Some(port) => {
            let (rx, addr) = wp_control::tcp::listen(port).map_err(|d| vec![d])?;
            log(&format!("control protocol listening on {addr}"));
            Some(rx)
        }
        None => None,
    };
    run(
        session,
        WindowOptions {
            title: project.game.value.name.clone(),
            width: 1280,
            height: 720,
            camera: wp_render::CameraMode::Chase,
            exit_after_s,
            control,
            quality: Some(tier),
            audio: true,
            governor: true,
            thermal: None,
        },
    )
    .map_err(|d| vec![d])
}

/// Write a line where the platform shows logs (stderr, or logcat on Android).
pub fn log(msg: &str) {
    #[cfg(target_os = "android")]
    android::log(msg);
    #[cfg(not(target_os = "android"))]
    eprintln!("{msg}");
}

#[cfg(target_os = "android")]
mod android {
    use std::ffi::CString;
    use std::io::Read;
    use std::path::PathBuf;
    use winit::platform::android::activity::AndroidApp;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(
            prio: i32,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> i32;
    }

    pub fn log(msg: &str) {
        const INFO: i32 = 4;
        let tag = c"waypoint";
        for line in msg.lines() {
            if let Ok(text) = CString::new(line) {
                // SAFETY: both pointers are valid NUL-terminated strings for the call.
                unsafe { __android_log_write(INFO, tag.as_ptr(), text.as_ptr()) };
            }
        }
    }

    /// Copy the game data from the APK's assets/game into app storage, using the
    /// file list the build script writes to assets/game/files.txt.
    pub fn unpack_game(app: &AndroidApp) -> Result<PathBuf, String> {
        let assets = app.asset_manager();
        let read = |name: &str| -> Result<Vec<u8>, String> {
            let c = CString::new(name).map_err(|e| e.to_string())?;
            let mut asset = assets
                .open(&c)
                .ok_or_else(|| format!("asset {name} is missing from the APK"))?;
            let mut bytes = Vec::new();
            asset
                .read_to_end(&mut bytes)
                .map_err(|e| format!("cannot read asset {name}: {e}"))?;
            Ok(bytes)
        };
        let base = app
            .internal_data_path()
            .ok_or("no internal data path")?
            .join("game");
        let list = String::from_utf8(read("game/files.txt")?).map_err(|e| e.to_string())?;
        for rel in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let dest = base.join(rel);
            std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&dest, read(&format!("game/{rel}"))?).map_err(|e| e.to_string())?;
        }
        Ok(base)
    }

    #[unsafe(no_mangle)]
    fn android_main(app: AndroidApp) {
        std::panic::set_hook(Box::new(|info| log(&format!("panic: {info}"))));
        let root = match unpack_game(&app) {
            Ok(r) => r,
            Err(e) => return log(&format!("error: {e}")),
        };
        log(&format!("playing {}", root.display()));
        let data = root.parent().map(std::path::Path::to_path_buf);
        let result = super::play(&root, data.as_deref(), None, None, |session, opts| {
            wp_platform::run_windowed_android(app, session, opts)
        });
        match result {
            Ok(summary) => log(&format!("exited after {} frames", summary.frames)),
            Err(diags) => {
                for d in diags {
                    log(&d.render());
                }
            }
        }
    }
}

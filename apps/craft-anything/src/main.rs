//! The Craft Anything desktop app.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use craft_anything_ai::AiHub;
use craft_anything_engine::{Engine, Session, Settings};
use craft_anything_ui_egui::CraftApp;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("craft-anything {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // A path on the command line (or from "Open with…") opens as the first target.
    let open: Option<PathBuf> = args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from);
    // The window app has no console: what goes wrong before (or instead of) a window is kept in
    // desktop.log beside the settings, so a start that shows nothing leaves a trace.
    log_to_file();
    log::info!("starting {} (pid {})", env!("CARGO_PKG_VERSION"), std::process::id());

    let settings = Settings::load();
    let mut session = Session::new(settings);
    let plugins = craft_anything_engine::plugins::install(&mut session);
    let mut layer_tools = craft_anything_ai::tools();
    layer_tools.extend(craft_anything_setup::tools::specs());
    let engine = Engine::shared_with(session, layer_tools, plugins);
    let hub = Arc::new(AiHub::new(engine.clone()));
    log::info!("engine and AI ready; opening the window");

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Craft Anything")
        .with_app_id("ai.storyteller.craft-anything")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([760.0, 520.0])
        // Our own title bar draws the window controls, moves and resizes the window.
        .with_decorations(false)
        .with_resizable(true)
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(craft_anything_ui_egui::APP_ICON_PNG) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let run = eframe::run_native(
        "Craft Anything",
        options,
        Box::new(move |cc| {
            log::info!("window open");
            CraftApp::setup_context(&cc.egui_ctx);
            let mut app = CraftApp::new(engine, hub);
            app.load_prefs(cc.storage);
            if let Some(p) = open {
                app.open_path(p);
            }
            Ok(Box::new(app))
        }),
    );
    match &run {
        Ok(()) => log::info!("closed"),
        Err(e) => log::error!("the window couldn't start: {e}"),
    }
    run
}

/// Kept small: started afresh once it passes this size.
const LOG_MAX: u64 = 512 * 1024;

/// Warnings and errors from everything, plus our own progress, appended to `desktop.log`; and
/// panics, which would otherwise vanish with the window.
fn log_to_file() {
    use std::io::Write as _;
    struct FileLog(std::sync::Mutex<Option<std::fs::File>>);
    impl log::Log for FileLog {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Warn || (m.level() <= log::Level::Info && m.target().starts_with("craft_anything"))
        }
        fn log(&self, r: &log::Record<'_>) {
            if !self.enabled(r.metadata()) {
                return;
            }
            let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if let Ok(mut f) = self.0.lock()
                && let Some(f) = f.as_mut()
            {
                let _ = writeln!(f, "{at} [{}] {}: {}", r.level(), r.target(), r.args());
            }
        }
        fn flush(&self) {}
    }
    let dir = craft_anything_engine::settings::config_dir();
    let path = dir.join("desktop.log");
    let _ = std::fs::create_dir_all(&dir);
    let fresh = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX);
    let file = std::fs::OpenOptions::new().create(true).append(!fresh).write(true).truncate(fresh).open(&path).ok();
    if log::set_logger(Box::leak(Box::new(FileLog(std::sync::Mutex::new(file))))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));
}

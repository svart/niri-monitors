use eframe::egui;
use niri_monitors::GuiApp;

/// Wayland app_id. Must match the installed desktop entry and icon name
/// (`contrib/desktop/niri-monitors.{desktop,svg}`) so launchers and niri can
/// associate the window with them. eframe also derives the storage directory
/// from it.
const APP_ID: &str = "niri-monitors";

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_inner_size([1200.0, 900.0])
            .with_min_inner_size([640.0, 480.0]),
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "Niri Monitors",
        native_options,
        Box::new(|cc| Ok(Box::new(GuiApp::new(cc)))),
    )
}

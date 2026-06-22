use niri_monitors::gui::app::GuiApp;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 900.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Monitoradlo",
        native_options,
        Box::new(|cc| Ok(Box::new(GuiApp::new(cc)))),
    )
}

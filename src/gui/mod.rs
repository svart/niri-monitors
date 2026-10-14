pub mod app;
pub mod canvas;
pub mod daemon_client;
pub mod output_editor;
pub mod poller;
pub mod profile_list;

use eframe::egui::Color32;

// Status colors shared by the GUI panels.
const OK_COLOR: Color32 = Color32::from_rgb(72, 143, 92);
const WARNING_COLOR: Color32 = Color32::from_rgb(184, 111, 36);
const ERROR_COLOR: Color32 = Color32::from_rgb(184, 67, 67);

pub mod config;
pub mod control;
pub mod daemon;
#[cfg(feature = "gui")]
mod gui;
pub mod matching;
pub mod model;
pub mod niri;
pub mod placement;

// The GUI is private so rustc reports GUI code that the binary does not use.
#[cfg(feature = "gui")]
pub use gui::app::GuiApp;

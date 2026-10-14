//! Colors and widget styling for light and dark mode.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Margin, RichText, Shadow, Stroke, TextStyle, Theme,
    ThemePreference, Visuals, WidgetText,
};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Header, sidebar, and inspector background.
    pub window: Color32,
    /// Cards inside panels, popups, and dialogs.
    pub surface: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub border: Color32,
    pub canvas: Color32,
    pub grid: Color32,
    pub tile: Color32,
    pub tile_border: Color32,
    /// Fill for outputs that are off or not connected.
    pub tile_off: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub shadow: Color32,
}

pub const DARK: Palette = Palette {
    window: Color32::from_rgb(31, 32, 36),
    surface: Color32::from_rgb(39, 41, 46),
    control: Color32::from_rgb(47, 50, 56),
    control_hover: Color32::from_rgb(58, 62, 70),
    control_active: Color32::from_rgb(69, 74, 84),
    border: Color32::from_rgb(56, 59, 66),
    canvas: Color32::from_rgb(24, 25, 28),
    grid: Color32::from_rgb(46, 48, 54),
    tile: Color32::from_rgb(45, 49, 57),
    tile_border: Color32::from_rgb(86, 92, 105),
    tile_off: Color32::from_rgb(34, 36, 41),
    text: Color32::from_rgb(230, 231, 234),
    text_weak: Color32::from_rgb(154, 158, 168),
    accent: Color32::from_rgb(76, 141, 255),
    on_accent: Color32::WHITE,
    ok: Color32::from_rgb(63, 178, 127),
    warning: Color32::from_rgb(230, 162, 60),
    error: Color32::from_rgb(240, 96, 90),
    shadow: Color32::from_black_alpha(110),
};

pub const LIGHT: Palette = Palette {
    window: Color32::from_rgb(246, 247, 249),
    surface: Color32::WHITE,
    control: Color32::from_rgb(236, 238, 242),
    control_hover: Color32::from_rgb(226, 229, 234),
    control_active: Color32::from_rgb(214, 218, 225),
    border: Color32::from_rgb(217, 220, 226),
    canvas: Color32::from_rgb(236, 238, 242),
    grid: Color32::from_rgb(208, 212, 219),
    tile: Color32::WHITE,
    tile_border: Color32::from_rgb(174, 180, 191),
    tile_off: Color32::from_rgb(228, 230, 234),
    text: Color32::from_rgb(29, 31, 36),
    text_weak: Color32::from_rgb(99, 105, 117),
    accent: Color32::from_rgb(37, 99, 235),
    on_accent: Color32::WHITE,
    ok: Color32::from_rgb(31, 143, 95),
    warning: Color32::from_rgb(178, 106, 0),
    error: Color32::from_rgb(209, 52, 47),
    shadow: Color32::from_black_alpha(40),
};

pub fn palette(ctx: &egui::Context) -> &'static Palette {
    match ctx.theme() {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// Installs both palettes and follows the system light or dark setting.
pub fn install(ctx: &egui::Context) {
    for (theme, palette) in [(Theme::Dark, &DARK), (Theme::Light, &LIGHT)] {
        ctx.set_visuals_of(theme, visuals(theme, palette));
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size.y = 28.0;
            style.spacing.combo_width = 160.0;
            for (text_style, size) in [
                (TextStyle::Heading, 18.0),
                (TextStyle::Body, 14.0),
                (TextStyle::Button, 14.0),
                (TextStyle::Small, 11.5),
            ] {
                style
                    .text_styles
                    .insert(text_style, FontId::proportional(size));
            }
            style
                .text_styles
                .insert(TextStyle::Monospace, FontId::monospace(13.0));
        });
    }
    ctx.set_theme(ThemePreference::System);
}

fn visuals(theme: Theme, palette: &Palette) -> Visuals {
    let mut visuals = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };
    visuals.panel_fill = palette.window;
    visuals.window_fill = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.extreme_bg_color = palette.canvas;
    visuals.hyperlink_color = palette.accent;
    visuals.warn_fg_color = palette.warning;
    visuals.error_fg_color = palette.error;
    visuals.selection.bg_fill = palette.accent;
    visuals.selection.stroke = Stroke::new(1.0_f32, palette.on_accent);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0_f32, palette.border);
    visuals.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: palette.shadow,
    };
    visuals.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: palette.shadow,
    };

    let widgets = &mut visuals.widgets;
    for (widget, fill) in [
        (&mut widgets.noninteractive, palette.window),
        (&mut widgets.inactive, palette.control),
        (&mut widgets.hovered, palette.control_hover),
        (&mut widgets.active, palette.control_active),
        (&mut widgets.open, palette.control_hover),
    ] {
        widget.corner_radius = CornerRadius::same(4);
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.fg_stroke = Stroke::new(1.0_f32, palette.text);
    }
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, palette.border);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, palette.accent);
    visuals
}

/// Frame for the header and side panels.
pub fn panel_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.window)
        .inner_margin(Margin::same(12))
}

/// Card that groups related settings, with a small uppercase title.
pub fn section<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    card(ui, |ui| {
        ui.label(
            RichText::new(title.to_uppercase())
                .small()
                .strong()
                .color(palette(ui.ctx()).text_weak),
        );
        ui.add_space(4.0);
        add_contents(ui)
    })
}

/// Card without a title, for a single setting that names itself.
pub fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let palette = palette(ui.ctx());
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// The single most important action in a view.
pub fn primary_button(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'static> {
    let palette = palette(ui.ctx());
    egui::Button::new(RichText::new(text).color(palette.on_accent)).fill(palette.accent)
}

/// A button for an action that deletes something.
pub fn danger_button(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'static> {
    let palette = palette(ui.ctx());
    egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(palette.error)
}

pub fn weak(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(palette(ui.ctx()).text_weak)
}

/// How a status is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Ok,
    Warning,
    Error,
}

impl Tone {
    pub fn color(self, palette: &Palette) -> Color32 {
        match self {
            Self::Neutral => palette.text_weak,
            Self::Ok => palette.ok,
            Self::Warning => palette.warning,
            Self::Error => palette.error,
        }
    }
}

/// Rounded label with a colored dot, for showing a state at a glance.
pub fn pill(ui: &mut egui::Ui, tone: Tone, text: impl Into<WidgetText>) -> egui::Response {
    let palette = palette(ui.ctx());
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                status_dot(ui, tone.color(palette));
                ui.label(text);
            });
        })
        .response
}

pub fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

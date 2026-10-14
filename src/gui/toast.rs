use crate::gui::theme::{self, Tone};
use eframe::egui::{self, CornerRadius, Margin, Stroke};
use std::time::{Duration, Instant};

const SUCCESS_DURATION: Duration = Duration::from_secs(3);
const ERROR_DURATION: Duration = Duration::from_secs(8);
const MAX_TOASTS: usize = 4;

/// Short messages that confirm an action or report its failure, shown in a corner
/// and dismissed on their own.
#[derive(Default)]
pub struct Toasts {
    toasts: Vec<Toast>,
}

struct Toast {
    tone: Tone,
    text: String,
    shown_at: Instant,
}

impl Toast {
    fn duration(&self) -> Duration {
        match self.tone {
            Tone::Error | Tone::Warning => ERROR_DURATION,
            Tone::Neutral | Tone::Ok => SUCCESS_DURATION,
        }
    }
}

impl Toasts {
    pub fn success(&mut self, text: impl Into<String>) {
        self.push(Tone::Ok, text.into());
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Tone::Error, text.into());
    }

    fn push(&mut self, tone: Tone, text: String) {
        self.toasts.retain(|toast| toast.text != text);
        self.toasts.push(Toast {
            tone,
            text,
            shown_at: Instant::now(),
        });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|toast| toast.shown_at.elapsed() < toast.duration());
        if self.toasts.is_empty() {
            return;
        }

        let palette = theme::palette(ctx);
        let mut dismissed = None;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                for (index, toast) in self.toasts.iter().enumerate() {
                    let response = egui::Frame::new()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0_f32, toast.tone.color(palette)))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(12, 8))
                        .shadow(ctx.style().visuals.popup_shadow)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                theme::status_dot(ui, toast.tone.color(palette));
                                ui.label(&toast.text);
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_text("Click to dismiss");
                    if response.clicked() {
                        dismissed = Some(index);
                    }
                }
            });
        if let Some(index) = dismissed {
            self.toasts.remove(index);
        }
    }
}

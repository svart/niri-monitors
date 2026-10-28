use crate::gui::daemon_client::DaemonConnectionState;
use crate::gui::theme::{self, Tone};
use eframe::egui::{self, RichText};

/// What the header says about the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSummary {
    pub tone: Tone,
    pub text: String,
    pub detail: String,
}

pub fn daemon_summary(state: &DaemonConnectionState) -> DaemonSummary {
    let status = match state {
        DaemonConnectionState::Checking => {
            return DaemonSummary {
                tone: Tone::Neutral,
                text: "Connecting to daemon…".to_owned(),
                detail: "Looking for niri-monitorsd on its control socket.".to_owned(),
            };
        }
        DaemonConnectionState::Offline { message } => {
            return DaemonSummary {
                tone: Tone::Warning,
                text: "Daemon offline".to_owned(),
                detail: format!(
                    "{message}\n\nStart niri-monitorsd to apply profiles when monitors change \
                     and to preview plans."
                ),
            };
        }
        DaemonConnectionState::Running(status) => status,
    };

    let (tone, mode, explanation) = if status.manual_layout {
        (
            Tone::Neutral,
            "Manual layout",
            "A layout arranged in this window is active, so profiles are not applied. \
             Saving the config returns the daemon to its configured mode.",
        )
    } else if status.manual_profile.is_some() {
        (
            Tone::Neutral,
            "Manual profile",
            "The daemon keeps the profile you activated and does not switch on its own.",
        )
    } else if status.auto_apply {
        (
            Tone::Ok,
            "Automatic",
            "The daemon applies the best matching profile when monitors change.",
        )
    } else {
        (
            Tone::Neutral,
            "Paused",
            "The daemon selects profiles but does not apply them.",
        )
    };

    let active = status.selected_profile.as_deref();
    let text = match active {
        Some(name) => format!("{mode} · {name}"),
        None => mode.to_owned(),
    };

    if let Some(last_apply) = status.last_apply.as_ref().filter(|apply| !apply.success) {
        return DaemonSummary {
            tone: Tone::Error,
            text,
            detail: format!(
                "Applying {} failed: {}",
                last_apply.profile,
                last_apply.error.as_deref().unwrap_or("no details")
            ),
        };
    }

    let detail = if active.is_none() && !status.manual_layout {
        format!("{explanation}\n\nNo profile matches the connected monitors.")
    } else {
        explanation.to_owned()
    };
    DaemonSummary { tone, text, detail }
}

/// What the user did in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderAction {
    Apply,
    Save,
    ToggleTheme,
}

/// Draws the header and returns what the user did.
///
/// `apply` is `None` to hide the Apply button, or why it is disabled.
pub fn show_header(
    ui: &mut egui::Ui,
    daemon: &DaemonSummary,
    niri_error: Option<&str>,
    dirty: bool,
    can_save: bool,
    apply: Option<Result<(), &str>>,
) -> Option<HeaderAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Monitors").heading().strong());
        ui.add_space(8.0);
        theme::pill(ui, daemon.tone, &daemon.text).on_hover_text(&daemon.detail);
        if let Some(error) = niri_error {
            theme::pill(ui, Tone::Error, "niri unreachable").on_hover_text(error);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::theme_toggle(ui).clicked() {
                action = Some(HeaderAction::ToggleTheme);
            }
            ui.add_space(4.0);
            let save = ui
                .add_enabled(can_save, theme::primary_button(ui, "Save"))
                .on_hover_text("Write the config file (Ctrl+S)")
                .on_disabled_hover_text(if dirty {
                    "Fix the problem shown below the header first"
                } else {
                    "No unsaved changes"
                });
            if save.clicked() {
                action = Some(HeaderAction::Save);
            }
            if let Some(apply) = apply {
                let button = ui
                    .add_enabled(apply.is_ok(), egui::Button::new("Apply"))
                    .on_hover_text("Apply the profile to the monitors without saving it");
                let button = match apply {
                    Ok(()) => button,
                    Err(reason) => button.on_disabled_hover_text(reason),
                };
                if button.clicked() {
                    action = Some(HeaderAction::Apply);
                }
            }
            if dirty {
                ui.label(RichText::new("Unsaved changes").color(palette.warning));
            }
        });
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::StatusData;
    use crate::daemon::LastApply;

    #[test]
    fn running_daemon_shows_mode_and_active_profile_name() {
        let summary = daemon_summary(&running(status(true, Some("Docked"))));

        assert_eq!(summary.tone, Tone::Ok);
        assert_eq!(summary.text, "Automatic · Docked");
    }

    #[test]
    fn manual_layout_wins_over_other_modes() {
        let mut status = status(false, None);
        status.manual_layout = true;

        let summary = daemon_summary(&running(status));

        assert_eq!(summary.text, "Manual layout");
    }

    #[test]
    fn failed_apply_is_an_error_with_its_reason() {
        let mut status = status(true, Some("Docked"));
        status.last_apply = Some(LastApply::failed(
            "Docked".to_owned(),
            Vec::new(),
            "mode 1920x1080 is not offered",
        ));

        let summary = daemon_summary(&running(status));

        assert_eq!(summary.tone, Tone::Error);
        assert!(summary.detail.contains("mode 1920x1080 is not offered"));
    }

    #[test]
    fn offline_daemon_explains_why() {
        let summary = daemon_summary(&DaemonConnectionState::Offline {
            message: "socket not found".to_owned(),
        });

        assert_eq!(summary.tone, Tone::Warning);
        assert!(summary.detail.starts_with("socket not found"));
    }

    fn running(status: StatusData) -> DaemonConnectionState {
        DaemonConnectionState::Running(status)
    }

    fn status(auto_apply: bool, selected_profile: Option<&str>) -> StatusData {
        StatusData {
            auto_apply,
            selected_profile: selected_profile.map(str::to_owned),
            manual_profile: None,
            manual_layout: false,
            outputs: Vec::new(),
            last_apply: None,
        }
    }
}

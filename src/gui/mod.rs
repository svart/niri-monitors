pub mod app;
pub mod canvas;
pub mod daemon_client;
pub mod header;
pub mod inspector;
pub mod poller;
pub mod profile_list;
pub mod sidebar;
pub mod theme;
pub mod toast;

/// Scale as people write it: `1×`, `1.25×`, `1.5×`.
fn format_scale(scale: f64) -> String {
    let text = format!("{scale:.3}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_scale_drops_trailing_zeros() {
        assert_eq!(format_scale(1.0), "1×");
        assert_eq!(format_scale(1.25), "1.25×");
        assert_eq!(format_scale(1.5), "1.5×");
    }
}

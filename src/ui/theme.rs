//! Visual styling for the console.
//!
//! The look borrows from the classic Microsoft Management Console: a quiet
//! neutral palette, tight row spacing, square corners, and a clear structural
//! divide between the scope pane and the result pane. Colour is reserved for
//! status, never decoration, which also keeps the contrast budget available
//! where it carries meaning.

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Selection blue, used for the selected row in whichever pane has focus.
pub const ACCENT: Color32 = Color32::from_rgb(0, 90, 158);
/// The same selection, muted, for panes that do not have focus. Keeping the
/// selection visible in every pane is what makes F6 comprehensible.
pub const ACCENT_INACTIVE: Color32 = Color32::from_rgb(74, 84, 94);
/// The ring drawn around the focused pane.
pub const FOCUS_RING: Color32 = Color32::from_rgb(0, 120, 212);

pub const OK: Color32 = Color32::from_rgb(16, 124, 16);
pub const WARN: Color32 = Color32::from_rgb(157, 93, 0);
pub const BAD: Color32 = Color32::from_rgb(168, 34, 34);
pub const MUTED: Color32 = Color32::from_rgb(110, 118, 128);

/// Height of a row in the result list. Deliberately compact — density is the
/// point of a management console.
pub const ROW_HEIGHT: f32 = 22.0;

pub fn apply(ctx: &egui::Context) {
    // The console is a light-mode tool: MMC's information density depends on
    // hairline separators and banding that do not survive a dark inversion.
    ctx.set_theme(egui::ThemePreference::Light);

    let mut visuals = Visuals::light();
    visuals.panel_fill = Color32::from_rgb(246, 246, 246);
    visuals.window_fill = Color32::from_rgb(252, 252, 252);
    visuals.extreme_bg_color = Color32::WHITE;
    visuals.faint_bg_color = Color32::from_rgb(242, 244, 246);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(214, 216, 219));
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(200, 203, 207));

    // Square everything off. Rounded corners read as "web app", not "console".
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::ZERO;
    }
    visuals.window_corner_radius = CornerRadius::same(2);
    visuals.menu_corner_radius = CornerRadius::same(2);

    // A visible focus ring is the whole basis of keyboard usability here, so it
    // is drawn more strongly than egui's default.
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, FOCUS_RING);

    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.indent = 16.0;
        style.spacing.scroll.bar_width = 10.0;
    });
}

/// Colour for a compliance or enablement status string.
pub fn status_color(text: &str) -> Color32 {
    match text {
        "Enabled" | "Compliant" | "Yes" => OK,
        "In grace period" | "Unknown" | "Conflict" => WARN,
        "Disabled" | "Not compliant" | "Error" => BAD,
        _ => MUTED,
    }
}

/// Colour for a licence usage bar: green with headroom, amber when nearly out,
/// red when fully consumed or over-assigned.
pub fn usage_color(fraction: f32) -> Color32 {
    if fraction >= 1.0 {
        BAD
    } else if fraction >= 0.9 {
        WARN
    } else {
        OK
    }
}

//! Keyboard handling for the whole console.
//!
//! Two rules keep this predictable:
//!
//! 1. **A focused text field wins.** When the filter box has focus, only Escape
//!    and the pane-cycling keys are intercepted; everything else is typing.
//! 2. **Keys are consumed, not observed.** Every shortcut uses `consume_key`, so
//!    a keystroke that drives navigation never also reaches a widget underneath.
//!
//! Shortcuts are listed for the user in [`super::help`]; that table and this
//! function are meant to be read side by side.

use egui::{Key, Modifiers};

use super::{App, Pane, View, nav};
use crate::worker::Command;

/// The order F6 cycles through. Details is skipped when the pane is hidden.
fn pane_order(show_details: bool) -> &'static [Pane] {
    if show_details {
        &[Pane::Nav, Pane::List, Pane::Details]
    } else {
        &[Pane::Nav, Pane::List]
    }
}

fn cycle_pane(app: &mut App, forward: bool) {
    let order = pane_order(app.show_details);
    let current = order.iter().position(|p| *p == app.pane).unwrap_or(0);
    let next = if forward {
        (current + 1) % order.len()
    } else {
        (current + order.len() - 1) % order.len()
    };
    app.pane = order[next];
}

pub fn handle(app: &mut App, ctx: &egui::Context) {
    // The help overlay is modal: it swallows keys so Escape always closes it.
    if app.show_help {
        let dismissed = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Escape)
                || i.consume_key(Modifiers::NONE, Key::F1)
                || i.consume_key(Modifiers::NONE, Key::Questionmark)
        });
        if dismissed {
            app.show_help = false;
        }
        return;
    }

    // Whether a text field currently owns the keyboard.
    let editing = ctx.memory(|m| m.focused().is_some())
        && ctx.egui_wants_keyboard_input();

    // ---- Always available, even while typing in the filter box -------------

    // The input closure cannot borrow `app`, so every shortcut is collected in
    // one pass and acted on afterwards.
    let pressed = ctx.input_mut(|i| Pressed {
        pane_forward: i.consume_key(Modifiers::NONE, Key::F6)
            || i.consume_key(Modifiers::COMMAND, Key::Tab),
        pane_back: i.consume_key(Modifiers::SHIFT, Key::F6)
            || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab),
        escape: i.consume_key(Modifiers::NONE, Key::Escape),
        refresh: i.consume_key(Modifiers::NONE, Key::F5),
        refresh_all: i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::R),
        help: i.consume_key(Modifiers::NONE, Key::F1),
        toggle_details: i.consume_key(Modifiers::COMMAND, Key::D),
        find: i.consume_key(Modifiers::COMMAND, Key::F),
        copy: i.consume_key(Modifiers::COMMAND, Key::C),
        write_mode: i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::W),
        mark_all: i.consume_key(Modifiers::COMMAND, Key::A),
        new_user: i.consume_key(Modifiers::COMMAND, Key::N),
        import_csv: i.consume_key(Modifiers::COMMAND, Key::I),
        export_csv: i.consume_key(Modifiers::COMMAND, Key::E),
        export_json: i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::E),
        // Shift+F10 is the long-standing "open the context menu for whatever is
        // focused" binding; Ctrl+Enter is the fallback for keyboards where F10
        // is claimed by the window manager.
        actions: i.consume_key(Modifiers::SHIFT, Key::F10)
            || i.consume_key(Modifiers::NONE, Key::F10)
            || i.consume_key(Modifiers::COMMAND, Key::Enter),
        clear_marks: i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::A),
        jump: jump_key(i),
    });

    if pressed.pane_forward {
        cycle_pane(app, true);
        release_text_focus(ctx);
    }
    if pressed.pane_back {
        cycle_pane(app, false);
        release_text_focus(ctx);
    }
    if pressed.help {
        app.show_help = true;
    }
    if pressed.toggle_details {
        app.show_details = !app.show_details;
        if !app.show_details && app.pane == Pane::Details {
            app.pane = Pane::List;
        }
    }
    if pressed.refresh {
        app.refresh_current();
    }
    if pressed.refresh_all {
        app.store.requested.clear();
        app.send(Command::LoadAll);
    }
    if pressed.find {
        app.focus_filter = true;
        app.pane = Pane::List;
    }
    if pressed.clear_marks {
        let view = app.view;
        app.view_state(view).clear_marks();
    } else if pressed.mark_all {
        let view = app.view;
        app.view_state(view).mark_all_filtered();
    }
    if pressed.new_user {
        app.new_user();
    }
    if pressed.import_csv {
        app.open_import();
    }
    if pressed.export_csv {
        app.export(super::export::Format::Csv);
    }
    if pressed.export_json {
        app.export(super::export::Format::Json);
    }
    if pressed.actions {
        app.open_palette();
    }
    if pressed.write_mode {
        // Turning write mode off is immediate; turning it on asks first.
        if app.write_mode.is_armed() {
            app.set_write_mode(false);
        } else if app.writes_available {
            app.arming = true;
        } else {
            app.set_write_mode(true); // reports why it cannot
        }
    }
    if let Some(view) = pressed.jump {
        // Make sure a nested node's parent is open before jumping to it.
        if let Some(parent) = nav::parent_of(view) {
            app.expanded.insert(parent);
        }
        app.go_to(view);
        app.pane = Pane::Nav;
        release_text_focus(ctx);
    }

    if pressed.escape {
        if editing {
            // First Escape leaves the filter box, keeping the text.
            release_text_focus(ctx);
        } else {
            let view = app.view;
            let had_filter = !app.view_state(view).filter.is_empty();
            if had_filter {
                app.view_state(view).filter.clear();
            } else if app.pane != Pane::Nav {
                app.pane = Pane::Nav;
            }
        }
    }

    // ---- Suppressed while a text field owns the keyboard -------------------

    if editing {
        return;
    }

    if pressed.copy {
        let text = super::details::copy_text(app);
        if !text.is_empty() {
            ctx.copy_text(text);
            app.status = "Copied to clipboard".into();
        }
    }

    let nav_keys = ctx.input_mut(|i| NavKeys {
        up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
        down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
        left: i.consume_key(Modifiers::NONE, Key::ArrowLeft),
        right: i.consume_key(Modifiers::NONE, Key::ArrowRight),
        page_up: i.consume_key(Modifiers::NONE, Key::PageUp),
        page_down: i.consume_key(Modifiers::NONE, Key::PageDown),
        home: i.consume_key(Modifiers::NONE, Key::Home),
        end: i.consume_key(Modifiers::NONE, Key::End),
        enter: i.consume_key(Modifiers::NONE, Key::Enter),
        space: i.consume_key(Modifiers::NONE, Key::Space),
    });

    match app.pane {
        Pane::Nav => nav_pane(app, &nav_keys),
        Pane::List => list_pane(app, &nav_keys),
        Pane::Details => details_pane(app, &nav_keys),
    }
}

struct Pressed {
    pane_forward: bool,
    pane_back: bool,
    escape: bool,
    refresh: bool,
    refresh_all: bool,
    help: bool,
    toggle_details: bool,
    find: bool,
    copy: bool,
    write_mode: bool,
    mark_all: bool,
    new_user: bool,
    clear_marks: bool,
    import_csv: bool,
    export_csv: bool,
    export_json: bool,
    actions: bool,
    jump: Option<View>,
}

struct NavKeys {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    page_up: bool,
    page_down: bool,
    home: bool,
    end: bool,
    enter: bool,
    space: bool,
}

/// Ctrl+0 through Ctrl+9 jump straight to a node.
///
/// The order matches the scope tree top to bottom, so the number is a position
/// rather than something to memorise separately.
pub const JUMP_KEYS: &[(Key, View)] = &[
    (Key::Num0, View::Overview),
    (Key::Num1, View::Users),
    (Key::Num2, View::Groups),
    (Key::Num3, View::Roles),
    (Key::Num4, View::Devices),
    (Key::Num5, View::ManagedDevices),
    (Key::Num6, View::Licenses),
    (Key::Num7, View::Mailboxes),
    (Key::Num8, View::Teams),
    (Key::Num9, View::SignIns),
];

fn jump_key(input: &mut egui::InputState) -> Option<View> {
    JUMP_KEYS
        .iter()
        .find(|(key, _)| input.consume_key(Modifiers::COMMAND, *key))
        .map(|(_, view)| *view)
}

/// Drop egui's text focus so subsequent arrow keys drive the list.
fn release_text_focus(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.stop_text_input());
}

fn nav_pane(app: &mut App, keys: &NavKeys) {
    if keys.up {
        nav::move_cursor(app, -1);
    }
    if keys.down {
        nav::move_cursor(app, 1);
    }
    if keys.right {
        nav::expand_or_enter(app);
    }
    if keys.left {
        nav::collapse_or_leave(app);
    }
    if keys.home {
        nav::move_cursor(app, i64::MIN / 2);
    }
    if keys.end {
        nav::move_cursor(app, i64::MAX / 2);
    }
    // Enter moves into the results, matching how MMC hands off from the tree.
    if keys.enter || keys.space {
        app.pane = Pane::List;
    }
}

fn list_pane(app: &mut App, keys: &NavKeys) {
    let view = app.view;
    if view == View::Overview {
        if keys.enter {
            app.pane = Pane::Nav;
        }
        return;
    }

    // A page is however many rows currently fit, less one for context.
    let page = {
        let state = app.view_state(view);
        ((state.last_viewport / super::theme::ROW_HEIGHT).floor() as i64 - 1).max(1)
    };

    let state = app.view_state(view);
    if keys.up {
        state.move_selection(-1);
    }
    if keys.down {
        state.move_selection(1);
    }
    if keys.page_up {
        state.move_selection(-page);
    }
    if keys.page_down {
        state.move_selection(page);
    }
    if keys.home {
        state.select_first();
    }
    if keys.end {
        state.select_last();
    }

    if keys.space {
        // Space ticks the row for a bulk operation, the convention in every
        // file manager, and moves on so a run can be ticked in one motion.
        let state = app.view_state(view);
        state.toggle_mark();
        state.move_selection(1);
    }
    if keys.enter {
        app.show_details = true;
        app.pane = Pane::Details;
    }
    if keys.left {
        app.pane = Pane::Nav;
    }
    if keys.right && app.show_details {
        app.pane = Pane::Details;
    }
}

fn details_pane(app: &mut App, keys: &NavKeys) {
    // The details pane is read-only, so arrows step the selection underneath it
    // — the same behaviour as a preview pane in a mail client.
    let view = app.view;
    if view == View::Overview {
        return;
    }
    let state = app.view_state(view);
    if keys.up {
        state.move_selection(-1);
    }
    if keys.down {
        state.move_selection(1);
    }
    if keys.left || keys.enter {
        app.pane = Pane::List;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_cycle_skips_hidden_details() {
        assert_eq!(pane_order(true).len(), 3);
        assert_eq!(pane_order(false), &[Pane::Nav, Pane::List]);
    }
}

//! What the console says when something is not available.
//!
//! A tenant that has not licensed Intune, an app registration that was never
//! granted `AuditLog.Read.All`, a mailbox nobody has rights over — these are
//! ordinary states, not failures, and a blank table is the worst possible way
//! to report one. So each gets a message.
//!
//! # The rule these follow
//!
//! **The joke is the headline. The fix is the body. They never swap places.**
//!
//! Every message here is a dry one-liner paired with the plain, factual
//! explanation that arrives from Graph — the permission that is missing, the
//! licence that is absent, the exact error. Somebody who finds the tone
//! irritating at four in the morning can ignore the top line entirely and still
//! have everything they need to fix the problem, which is the only reason the
//! top line is allowed to be funny at all.
//!
//! Collecting them here rather than scattering them through the panes is
//! deliberate: tone drifts when it lives in twelve places, and a file of jokes
//! is easy to read end to end and ask whether any of them have aged badly.

use super::View;

/// The headline for a view the tenant does not offer.
///
/// Aimed at the situation, never at the person reading it — they are usually
/// the one who has just discovered the licence was never bought.
pub fn unavailable(view: View) -> &'static str {
    match view {
        View::ManagedDevices => {
            "Intune has not been invited to this tenant"
        }
        View::SignIns => {
            "The sign-in log is being coy"
        }
        View::AuditLogs => {
            "Nobody here remembers doing anything"
        }
        View::Teams => {
            "Teams is not answering, which is a first"
        }
        View::Mailboxes => {
            "Exchange would rather not discuss its mailboxes"
        }
        // The remaining views come from the core directory. If those are
        // unavailable the sign-in itself has failed, and there is nothing
        // amusing about that.
        View::Overview
        | View::Users
        | View::Groups
        | View::Roles
        | View::Devices
        | View::Licenses => "This view is not available",
    }
}

/// The line under the headline: what the operator can actually do next.
///
/// Deliberately separate from the Graph error, which is shown verbatim below
/// both — this is the "so do X" sentence, in plain words.
pub fn remedy(view: View) -> &'static str {
    match view {
        View::ManagedDevices => {
            "Grant the app registration DeviceManagementManagedDevices.Read.All, or \
             accept that these laptops are somebody else's problem."
        }
        View::SignIns => {
            "This one needs Microsoft Entra ID P1, AuditLog.Read.All on the app \
             registration, and a reporting role on your own account. Three things, \
             all of them somebody's job."
        }
        View::AuditLogs => {
            "Grant the app registration AuditLog.Read.All and give your account a \
             reporting role. Until then the directory keeps its secrets."
        }
        View::Teams => {
            "Grant the app registration Team.ReadBasic.All. If Teams genuinely is not \
             licensed here, congratulations."
        }
        View::Mailboxes => {
            "This list comes from the mailbox usage report, so it needs \
             Reports.Read.All. Without it, Exchange is a rumour."
        }
        View::Overview
        | View::Users
        | View::Groups
        | View::Roles
        | View::Devices
        | View::Licenses => {
            "Check the permissions granted to the app registration, then sign out and \
             back in."
        }
    }
}

/// Headline for a detail that could not be read for one selected object, where
/// the rest of the view is working perfectly well.
pub fn detail_unavailable(what: Detail) -> &'static str {
    match what {
        Detail::TeamSettings => "This team is keeping its settings to itself",
        Detail::MailboxSettings => "This mailbox declined to be introduced",
    }
}

/// Which per-object detail could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    TeamSettings,
    MailboxSettings,
}

/// The line shown in the status bar when an action cannot be offered at all.
///
/// Shorter and drier than the pane messages: the status bar is glanced at, not
/// read, so this has one clause of tone and one of fact.
pub fn nothing_to_do(what: &str) -> String {
    format!("Nothing to do to {what} — this view is for looking at, not touching")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_VIEW: &[View] = &[
        View::Overview,
        View::Users,
        View::Groups,
        View::Roles,
        View::Devices,
        View::ManagedDevices,
        View::Licenses,
        View::Mailboxes,
        View::Teams,
        View::SignIns,
        View::AuditLogs,
    ];

    #[test]
    fn every_view_has_both_halves_of_the_message() {
        // A headline without a remedy is a joke at the operator's expense,
        // which is the one thing this module must never ship.
        for view in EVERY_VIEW {
            assert!(!unavailable(*view).is_empty(), "{view:?} has no headline");
            assert!(!remedy(*view).is_empty(), "{view:?} has no remedy");
        }
    }

    #[test]
    fn every_remedy_names_something_actionable() {
        // The point of the second line is that it tells somebody what to do.
        // A remedy that mentions no permission, licence or step is just more
        // tone, and this test is what stops one being written that way.
        const ACTIONABLE: &[&str] = &[
            "Grant",
            "grant",
            "needs",
            "Check",
            "sign out",
            "licensed",
        ];
        for view in EVERY_VIEW {
            let text = remedy(*view);
            assert!(
                ACTIONABLE.iter().any(|word| text.contains(word)),
                "{view:?}'s remedy tells nobody what to do: {text}"
            );
        }
    }

    #[test]
    fn the_core_directory_views_are_not_joked_about() {
        // If Users will not load, the operator has a real problem and is not in
        // the mood.
        for view in [View::Users, View::Groups, View::Roles, View::Licenses] {
            assert_eq!(unavailable(view), "This view is not available");
        }
    }
}

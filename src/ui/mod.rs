//! The Graphical Cloud Manager console.
//!
//! Three panes, left to right: a scope tree, a result list, and a details pane.
//! Focus moves between them with F6, and every pane is operable from the
//! keyboard alone — see [`keys`] for the full map.

mod confirm;
mod database;
mod details;
mod directory;
mod export;
mod forms;
mod help;
mod import;
mod keys;
mod list;
mod menu;
mod nav;
mod quips;
mod theme;
mod update;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, RichText};

use crate::config::{Config, source_label};
use crate::graph::Fetch;
use crate::graph::actions::{Action, Severity};
use crate::graph::models::*;
use crate::ldap::models::{AdComputer, AdUser};
use crate::worker::{Collection, Command, Event, Worker};

pub const FRIENDLY_NAME: &str = "Graphical Cloud Manager";

/// How long write mode survives without the operator touching anything.
///
/// The access token is write-capable for the whole session, so an armed console
/// left unattended is the real exposure. This bounds it.
const WRITE_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Describe the slice of the logs a view covers.
///
/// Both halves matter and neither implies the other: an operator who reads only
/// "last 7 days" will assume they are seeing all of it, and one who reads only
/// "most recent 500" will not know how far back that reaches.
fn describe_log_window(days: u32, records: usize) -> String {
    let period = match days {
        1 => "last 24 hours".to_string(),
        7 => "last 7 days".to_string(),
        other => format!("last {other} days"),
    };
    format!("{period}, most recent {records}")
}

/// Whether a failed database export looks like the password was wrong.
///
/// Worth getting right rather than always forgetting or never forgetting: a
/// remembered password that is wrong makes every retry fail identically without
/// ever offering to correct it, and forgetting a *correct* password because the
/// server was briefly unreachable means typing it again for no reason.
fn looks_like_a_credential_problem(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("access denied")
        || lowered.contains("authentication")
        // MySQL 1045 is access-denied-for-user; 1698 is a plugin refusing the
        // credential it was given.
        || lowered.contains("1045")
        || lowered.contains("1698")
}

/// How far through a batch the worker has got.
#[derive(Debug, Clone, Copy)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
}

/// Whether the console may change the tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Locked,
    Armed,
}

impl WriteMode {
    pub fn is_armed(self) -> bool {
        self == WriteMode::Armed
    }
}

/// Which collection the result pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Overview,
    Users,
    Groups,
    Roles,
    Devices,
    ManagedDevices,
    Licenses,
    Mailboxes,
    Teams,
    SignIns,
    AuditLogs,
    AdUsers,
    AdComputers,
}

impl View {
    /// True for the views served by a domain controller rather than by Graph.
    ///
    /// These need a bind password before they can load, which is the one thing
    /// that makes them behave differently from every other node in the tree.
    pub fn is_directory(self) -> bool {
        matches!(self, View::AdUsers | View::AdComputers)
    }

    pub fn title(self) -> &'static str {
        match self {
            View::Overview => "Console Root",
            View::Users => "Users",
            View::Groups => "Groups",
            View::Roles => "Directory Roles",
            View::Devices => "Entra Devices",
            View::ManagedDevices => "Managed Devices (Intune)",
            View::Licenses => "Licenses",
            View::Mailboxes => "Mailboxes (Exchange)",
            View::Teams => "Teams",
            View::SignIns => "Sign-in Logs",
            View::AuditLogs => "Audit Logs",
            View::AdUsers => "AD Users (on-premises)",
            View::AdComputers => "AD Computers (on-premises)",
        }
    }

    /// The collection to reload when the user presses F5 in this view.
    pub fn collection(self) -> Option<Collection> {
        match self {
            View::Overview => None,
            View::Users => Some(Collection::Users),
            View::Groups => Some(Collection::Groups),
            View::Roles => Some(Collection::Roles),
            View::Devices => Some(Collection::Devices),
            View::ManagedDevices => Some(Collection::ManagedDevices),
            View::Licenses => Some(Collection::Licenses),
            View::Mailboxes => Some(Collection::Mailboxes),
            View::Teams => Some(Collection::Teams),
            View::SignIns => Some(Collection::SignIns),
            View::AuditLogs => Some(Collection::AuditLogs),
            View::AdUsers => Some(Collection::AdUsers),
            View::AdComputers => Some(Collection::AdComputers),
        }
    }

    /// The database table this view is written to, without the configured
    /// prefix. `None` for the console root, which is a summary rather than a
    /// collection.
    ///
    /// Spelled out rather than derived from [`Self::title`] because a table
    /// name is part of somebody's schema: renaming a view's heading should
    /// never silently orphan the table their dashboard queries.
    pub fn table_stem(self) -> Option<&'static str> {
        match self {
            View::Overview => None,
            View::Users => Some("users"),
            View::Groups => Some("groups"),
            View::Roles => Some("roles"),
            View::Devices => Some("devices"),
            View::ManagedDevices => Some("managed_devices"),
            View::Licenses => Some("licenses"),
            View::Mailboxes => Some("mailboxes"),
            View::Teams => Some("teams"),
            View::SignIns => Some("sign_ins"),
            View::AuditLogs => Some("audit_logs"),
            View::AdUsers => Some("ad_users"),
            View::AdComputers => Some("ad_computers"),
        }
    }

    /// Every view that holds a collection, in the order they are exported.
    pub const ALL: &'static [View] = &[
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
        View::AdUsers,
        View::AdComputers,
    ];

    /// True for the views that read a log rather than a directory collection.
    ///
    /// These are bounded by time and record count rather than showing
    /// everything, so the header says so — a count that silently stops at 500
    /// would otherwise read as "that is all there is".
    pub fn is_log(self) -> bool {
        matches!(self, View::SignIns | View::AuditLogs)
    }
}

/// Which pane currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Nav,
    List,
    Details,
}

/// Where the application is in its lifecycle.
enum Phase {
    /// Redeeming a cached refresh token; nothing for the user to do.
    SigningInSilently,
    /// Waiting for the operator to finish signing in in their browser.
    AwaitingBrowser { url: String },
    Ready,
    /// Unrecoverable: bad configuration, or sign-in refused.
    Fatal(String),
}

/// Everything fetched from Graph, plus per-collection load state.
#[derive(Default)]
struct Store {
    org: Option<Organization>,
    users: Arc<Vec<User>>,
    groups: Arc<Vec<Group>>,
    roles: Arc<Vec<DirectoryRole>>,
    devices: Arc<Vec<Device>>,
    managed: Option<Fetch<Arc<Vec<ManagedDevice>>>>,
    licenses: Arc<Vec<SubscribedSku>>,
    sign_ins: Option<Fetch<Arc<Vec<SignIn>>>>,
    audits: Option<Fetch<Arc<Vec<DirectoryAudit>>>>,
    teams: Option<Fetch<Arc<Vec<Team>>>>,
    /// The usage report, exactly as it arrived. Not what the view shows — see
    /// `mailbox_rows`.
    mailboxes: Option<Fetch<Arc<Vec<Mailbox>>>>,
    /// What the Mailboxes view actually lists: the report's rows plus every
    /// mailbox `users` knows about that the report has not caught up with,
    /// ordered fullest first.
    ///
    /// Rebuilt by `set_mailboxes` and `set_users`, and the only list the view's
    /// row indices point into. Held separately from `mailboxes` so a refresh of
    /// either half cannot leave the union stale.
    mailbox_rows: Arc<Vec<Mailbox>>,
    /// On-premises accounts, when a domain controller is configured and has
    /// been bound to. `Unavailable` covers the ordinary cloud-only case.
    ad_users: Option<Fetch<Arc<Vec<AdUser>>>>,
    ad_computers: Option<Fetch<Arc<Vec<AdComputer>>>>,
    /// AD users indexed by lower-cased sAMAccountName, rebuilt whenever the AD
    /// user list is replaced. This is what joins the two directories together
    /// in the Entra Users pane; building it once beats scanning the whole AD
    /// list on every frame the selection sits on a synced account.
    ad_by_sam: HashMap<String, usize>,

    /// Lazily loaded details, keyed by object id.
    group_members: HashMap<String, (Arc<Vec<DirectoryMember>>, Arc<Vec<DirectoryMember>>)>,
    role_members: HashMap<String, Arc<Vec<DirectoryMember>>>,
    user_memberships: HashMap<String, Arc<Vec<DirectoryMember>>>,
    /// Full team settings and channels, keyed by team id.
    team_details: HashMap<String, (Fetch<Team>, Arc<Vec<Channel>>)>,
    /// Mailbox settings, keyed by whichever identifier the request used.
    mailbox_settings: HashMap<String, Fetch<MailboxSettings>>,
    /// Detail requests already in flight, so we ask only once per object.
    requested: HashSet<String>,

    loading: HashSet<Collection>,
    errors: HashMap<Collection, String>,
    /// Notice that is not tied to one collection (e.g. tenant read failed).
    notice: Option<String>,
    /// Bumped whenever a collection is replaced, to invalidate filter caches.
    version: u64,
}

impl Store {
    fn is_loading(&self, view: View) -> bool {
        view.collection()
            .map(|c| self.loading.contains(&c))
            .unwrap_or(false)
    }

    fn error(&self, view: View) -> Option<&String> {
        view.collection().and_then(|c| self.errors.get(&c))
    }

    /// Number of rows a view will show before filtering.
    fn count(&self, view: View) -> Option<usize> {
        /// A collection the tenant may not offer has no count until it arrives.
        fn ready<T>(fetch: &Option<Fetch<Arc<Vec<T>>>>) -> Option<usize> {
            match fetch {
                Some(Fetch::Ready(items)) => Some(items.len()),
                _ => None,
            }
        }

        match view {
            View::Overview => None,
            View::Users => Some(self.users.len()),
            View::Groups => Some(self.groups.len()),
            View::Roles => Some(self.roles.len()),
            View::Devices => Some(self.devices.len()),
            View::Licenses => Some(self.licenses.len()),
            View::ManagedDevices => ready(&self.managed),
            // The union, not the report — that is what the view lists.
            View::Mailboxes => match &self.mailboxes {
                Some(Fetch::Ready(_)) => Some(self.mailbox_rows.len()),
                _ => None,
            },
            View::Teams => ready(&self.teams),
            View::SignIns => ready(&self.sign_ins),
            View::AuditLogs => ready(&self.audits),
            View::AdUsers => ready(&self.ad_users),
            View::AdComputers => ready(&self.ad_computers),
        }
    }

    /// The reason a view has nothing to show, when the tenant does not offer it.
    ///
    /// One accessor rather than a `match` at every call site, so a new optional
    /// collection cannot be added and then silently render an empty table.
    fn unavailable(&self, view: View) -> Option<&str> {
        fn reason<T>(fetch: &Option<Fetch<T>>) -> Option<&str> {
            match fetch {
                Some(Fetch::Unavailable(reason)) => Some(reason.as_str()),
                _ => None,
            }
        }

        match view {
            View::ManagedDevices => reason(&self.managed),
            View::Mailboxes => reason(&self.mailboxes),
            View::Teams => reason(&self.teams),
            View::SignIns => reason(&self.sign_ins),
            View::AuditLogs => reason(&self.audits),
            View::AdUsers => reason(&self.ad_users),
            View::AdComputers => reason(&self.ad_computers),
            _ => None,
        }
    }

    /// Store the usage report *and* rebuild the list the view shows.
    ///
    /// Paired for the same reason [`Self::set_ad_users`] is: the view's row
    /// indices point into `mailbox_rows`, so assigning the report without
    /// rebuilding would leave the selection pointing at a different mailbox
    /// than the one under the cursor.
    fn set_mailboxes(&mut self, fetch: Fetch<Arc<Vec<Mailbox>>>) {
        self.mailboxes = Some(fetch);
        self.rebuild_mailbox_rows();
    }

    /// Store the user list *and* rebuild the mailbox list that draws on it.
    ///
    /// Only the authoritative list from Graph comes through here. The
    /// optimistic in-place edits elsewhere — a job title, an enable/disable —
    /// cannot change whether an account has a mailbox, and the one that could,
    /// a licence change, is followed by a reload of the whole collection.
    fn set_users(&mut self, users: Arc<Vec<User>>) {
        self.users = users;
        self.rebuild_mailbox_rows();
    }

    /// Union the usage report with the accounts that hold a live Exchange plan.
    ///
    /// Neither source is sufficient alone and they miss different things, which
    /// is why this unions rather than choosing. The report is a day stale but
    /// carries every number the view shows and covers the shared, room and
    /// equipment mailboxes that hold no licence. `/users` carries no numbers at
    /// all but is live, so a mailbox made this morning appears today instead of
    /// tomorrow.
    ///
    /// Rows the report supplied always win: a mailbox in both is the reported
    /// one, numbers and all.
    fn rebuild_mailbox_rows(&mut self) {
        // Only when the report actually arrived. A tenant with no Exchange, or
        // an app registration without Reports.Read.All, gets the explanation
        // the view already gives — a partial list with every figure blank would
        // be a worse answer than a clear account of why there is no list.
        let Some(Fetch::Ready(reported)) = &self.mailboxes else {
            self.mailbox_rows = Arc::new(Vec::new());
            return;
        };

        let mut rows: Vec<Mailbox> = reported.as_ref().clone();

        // A tenant that conceals report identities returns opaque GUIDs where
        // the UPN should be, so there is no key to join on. Joining anyway
        // would match nothing and append a duplicate of every mailbox under its
        // real name — worse than not joining at all.
        let concealed = rows.iter().any(Mailbox::is_concealed);
        if !concealed {
            let reported_upns: HashSet<String> = rows
                .iter()
                .map(|mailbox| mailbox.user_principal_name.to_lowercase())
                .collect();

            for user in self.users.iter() {
                let Some(upn) = user.user_principal_name.as_deref() else {
                    continue;
                };
                // Case folded on both sides, like the sAMAccountName join:
                // Entra echoes back whatever case the UPN was created with, and
                // the report has its own ideas.
                if !user.has_mailbox() || reported_upns.contains(&upn.to_lowercase()) {
                    continue;
                }
                rows.push(Mailbox::from_directory(user));
            }
        }

        // Fullest first, as before — the mailbox about to stop receiving mail
        // is what this view exists to surface. Rows with no figures yet sort
        // below every reported one rather than among the empty mailboxes, where
        // a nil usage bar would put them.
        rows.sort_by(|a, b| {
            b.metrics_known()
                .cmp(&a.metrics_known())
                .then_with(|| b.usage_fraction().total_cmp(&a.usage_fraction()))
                .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
        });

        self.mailbox_rows = Arc::new(rows);
    }

    /// Store the AD user list *and* rebuild the index that joins it to Entra.
    ///
    /// One method rather than two statements, because `ad_by_sam` holds
    /// positions into exactly this list. Assigning the list without rebuilding
    /// the index leaves the join silently answering from stale positions — or,
    /// on a first load, answering nothing at all while every synced account
    /// reports "no matching account found", which is indistinguishable from a
    /// genuinely absent object. Making the two inseparable is the only way that
    /// cannot be got wrong at a call site.
    ///
    /// The key is the lower-cased sAMAccountName: the one identifier both
    /// directories agree on. Entra carries it as `onPremisesSamAccountName` on
    /// every synced account, and it survives the UPN changes and mail-domain
    /// moves that break every other join. Case is folded because AD treats it
    /// case-insensitively and Entra echoes back whatever was set at sync time.
    fn set_ad_users(&mut self, fetch: Fetch<Arc<Vec<AdUser>>>) {
        self.ad_by_sam.clear();
        if let Fetch::Ready(users) = &fetch {
            for (index, user) in users.iter().enumerate() {
                if let Some(sam) = &user.sam_account_name {
                    self.ad_by_sam.insert(sam.to_lowercase(), index);
                }
            }
        }
        self.ad_users = Some(fetch);
    }

    /// Forget the AD user list *and* its index, so a refresh cannot leave
    /// positions behind pointing into a list that is no longer there.
    fn clear_ad_users(&mut self) {
        self.ad_by_sam.clear();
        self.ad_users = None;
    }

    /// The AD account behind a synced Entra user, if it has been read.
    ///
    /// Returns `None` for a cloud-only account, for a synced one whose AD
    /// object is outside the configured base DN, and whenever the Directory
    /// node has simply not been loaded — three different situations that the
    /// details pane distinguishes, and this deliberately does not.
    fn ad_match(&self, user: &User) -> Option<&AdUser> {
        let sam = user.on_premises_sam_account_name.as_ref()?;
        let index = *self.ad_by_sam.get(&sam.to_lowercase())?;
        match &self.ad_users {
            Some(Fetch::Ready(users)) => users.get(index),
            _ => None,
        }
    }

    /// The rows of an optional collection, or an empty slice while it is
    /// loading or unavailable.
    fn optional<T>(fetch: &Option<Fetch<Arc<Vec<T>>>>) -> Arc<Vec<T>> {
        match fetch {
            Some(Fetch::Ready(items)) => items.clone(),
            _ => Arc::new(Vec::new()),
        }
    }
}

/// Per-view list state: the filter box, the selection, and a cached mapping
/// from visible row to underlying object index.
#[derive(Default)]
pub struct ViewState {
    pub filter: String,
    pub selected: usize,
    pub filtered: Vec<usize>,
    /// Inputs the cached `filtered` was built from.
    cache_key: Option<(u64, String)>,
    /// Set when the selection changed via keyboard, so the list scrolls to it.
    pub scroll_to_selection: bool,
    /// Scroll offset and viewport height from the previous frame. Keeping them
    /// lets the next frame scroll to a row that is not currently rendered.
    pub last_offset: f32,
    pub last_viewport: f32,
    /// Rows ticked for a bulk operation, held as *source* indices so the set
    /// survives filtering — narrow the filter, tick more, widen it again, and
    /// the earlier ticks are still there.
    pub marked: BTreeSet<usize>,
}

impl ViewState {
    /// Rebuild `filtered` if the data or the filter text changed.
    fn refresh<F>(&mut self, version: u64, len: usize, matches: F)
    where
        F: Fn(usize, &str) -> bool,
    {
        let key = (version, self.filter.clone());
        if self.cache_key.as_ref() == Some(&key) {
            return;
        }

        let needle = self.filter.trim().to_lowercase();
        self.filtered = if needle.is_empty() {
            (0..len).collect()
        } else {
            (0..len).filter(|i| matches(*i, &needle)).collect()
        };
        self.cache_key = Some(key);
        // Keep the selection in range without snapping it to zero on every
        // keystroke, which would make type-to-filter unusable.
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    /// The underlying object index for the current selection.
    pub fn selected_source(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    pub fn move_selection(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let last = self.filtered.len() as i64 - 1;
        let next = (self.selected as i64 + delta).clamp(0, last);
        self.selected = next as usize;
        self.scroll_to_selection = true;
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.scroll_to_selection = true;
    }

    pub fn select_last(&mut self) {
        self.selected = self.filtered.len().saturating_sub(1);
        self.scroll_to_selection = true;
    }

    /// Tick or untick the row under the cursor.
    pub fn toggle_mark(&mut self) {
        if let Some(source) = self.selected_source()
            && !self.marked.insert(source)
        {
            self.marked.remove(&source);
        }
    }

    /// Tick everything the current filter shows. Rows hidden by the filter are
    /// deliberately left alone — "select all" should never reach past what the
    /// operator can see.
    pub fn mark_all_filtered(&mut self) {
        self.marked.extend(self.filtered.iter().copied());
    }

    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// True when the operator has ticked rows explicitly.
    pub fn has_marks(&self) -> bool {
        !self.marked.is_empty()
    }
}

pub struct App {
    worker: Option<Worker>,
    phase: Phase,
    store: Store,

    view: View,
    pane: Pane,
    /// Expanded parent nodes in the scope tree.
    expanded: HashSet<&'static str>,
    /// Index of the focused row in the scope tree.
    nav_cursor: usize,
    /// Per-view list state.
    views: HashMap<View, ViewState>,

    account: Option<String>,
    tenant_label: String,
    /// The window the log views cover, as configured. Kept as text because the
    /// only thing the UI does with it is say it out loud.
    log_window: String,
    show_help: bool,
    show_details: bool,
    /// Set when a shortcut asks for the filter box to take focus next frame.
    focus_filter: bool,
    status: String,

    /// Whether the console may change the tenant. Always starts `Locked`.
    write_mode: WriteMode,
    /// Whether the tenant granted the write scopes at sign-in. When false,
    /// write mode cannot be armed at all — the token could not write anyway.
    writes_available: bool,
    /// Last interaction while armed, for the idle timeout.
    last_activity: Instant,
    /// Waiting for the operator to approve arming write mode.
    arming: bool,
    /// An action awaiting confirmation.
    pending: Option<confirm::Pending>,
    /// An open form or picker gathering the details of an action.
    form: Option<forms::Form>,
    /// The keyboard route to the actions menu (Shift+F10).
    palette: Option<menu::Palette>,
    /// A parsed import awaiting the operator's approval.
    import: Option<crate::importer::Plan>,
    /// Where the database export writes, when it is configured at all.
    mariadb: Option<crate::config::MariaDb>,
    /// The database password, for as long as this session lasts.
    ///
    /// Deliberately not persisted, and cleared on sign-out along with
    /// everything else the session knows. Typing it once per run is the price
    /// of keeping the configuration file free of secrets.
    mariadb_password: Option<crate::mariadb::Secret>,
    /// The export dialog, while it is open.
    database: Option<database::Prompt>,
    /// The domain controller to read, when one is configured at all.
    directory: Option<crate::config::Directory>,
    /// The LDAP bind password, for as long as this session lasts. Held on the
    /// same terms as [`Self::mariadb_password`] and cleared at the same time.
    directory_password: Option<crate::mariadb::Secret>,
    /// The bind dialog, while it is open.
    directory_prompt: Option<directory::Prompt>,
    /// The view to load once a bind succeeds — the one whose selection opened
    /// the dialog in the first place.
    directory_pending: Option<Collection>,
    /// Tables written so far by the export in flight.
    database_progress: Vec<(String, usize)>,
    /// Outcome of the most recent action, shown in the status bar.
    last_action: Option<Result<String, String>>,
    /// A batch in flight.
    batch: Option<BatchProgress>,
    /// What failed in the last batch, shown until dismissed. A partial failure
    /// has to be read, not glimpsed in a status bar.
    batch_failures: Option<(usize, Vec<(String, String)>)>,
    /// A release newer than this build, offered once and then either applied
    /// or dismissed for the rest of the session.
    pending_update: Option<crate::update::Release>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, config: Config) -> Self {
        theme::apply(&cc.egui_ctx);

        let ctx = cc.egui_ctx.clone();
        let tenant_label = config.tenant_id().to_string();
        let log_window = describe_log_window(config.log_days(), config.log_records());
        let mariadb = config.mariadb.clone();
        let directory = config.directory.clone();
        let worker = Worker::spawn(config, move || ctx.request_repaint());

        let (worker, phase) = match worker {
            Ok(worker) => {
                worker.send(Command::SignIn);
                // Once per launch, off the back of sign-in rather than
                // gating on it: a tenant that is slow to answer should not
                // also delay finding out a new build exists.
                worker.send(Command::CheckForUpdate);
                (Some(worker), Phase::SigningInSilently)
            }
            Err(err) => (None, Phase::Fatal(format!("Could not start: {err}"))),
        };

        let mut expanded = HashSet::new();
        expanded.insert("groups");
        expanded.insert("devices");
        expanded.insert("directory");

        Self {
            worker,
            phase,
            store: Store::default(),
            view: View::Overview,
            pane: Pane::Nav,
            expanded,
            nav_cursor: 0,
            views: HashMap::new(),
            account: None,
            tenant_label,
            log_window,
            show_help: false,
            show_details: true,
            focus_filter: false,
            write_mode: WriteMode::Locked,
            writes_available: true,
            last_activity: Instant::now(),
            arming: false,
            pending: None,
            form: None,
            palette: None,
            import: None,
            mariadb,
            mariadb_password: None,
            database: None,
            directory,
            directory_password: None,
            directory_prompt: None,
            directory_pending: None,
            database_progress: Vec::new(),
            batch: None,
            batch_failures: None,
            pending_update: None,
            last_action: None,
            status: "Starting…".into(),
        }
    }

    /// Construct the console populated with synthetic data and no worker, for
    /// developing the interface without a tenant. Debug builds only.
    #[cfg(debug_assertions)]
    pub fn demo(cc: &eframe::CreationContext<'_>) -> Self {
        use crate::demo;

        theme::apply(&cc.egui_ctx);

        let mut store = Store {
            org: Some(demo::organization()),
            users: demo::users(),
            groups: demo::groups(),
            roles: demo::roles(),
            devices: demo::devices(),
            ad_computers: Some(demo::ad_computers()),
            managed: Some(demo::managed_devices()),
            licenses: demo::licenses(),
            mailboxes: Some(demo::mailboxes()),
            teams: Some(demo::teams()),
            sign_ins: Some(demo::sign_ins()),
            audits: Some(demo::audits()),
            ..Default::default()
        };

        // Pre-fill the lazily loaded detail caches so the details pane is not
        // stuck on a spinner that no worker will ever satisfy.
        for (index, group) in store.groups.iter().enumerate() {
            store.group_members.insert(
                group.id.clone(),
                (demo::members(4 + index, index), demo::members(1, index + 3)),
            );
        }
        for (index, role) in store.roles.iter().enumerate() {
            store
                .role_members
                .insert(role.id.clone(), demo::members(1 + index % 3, index));
        }
        for (index, user) in store.users.iter().enumerate() {
            store
                .user_memberships
                .insert(user.id.clone(), demo::members(2 + index % 3, index));
        }
        if let Some(Fetch::Ready(teams)) = &store.teams {
            let details: Vec<_> = teams
                .iter()
                .map(|team| (team.id.clone(), demo::team_detail(team)))
                .collect();
            store.team_details.extend(details);
        }
        if let Some(Fetch::Ready(mailboxes)) = &store.mailboxes {
            let settings: Vec<_> = mailboxes
                .iter()
                .map(|mailbox| {
                    let upn = mailbox.user_principal_name.clone();
                    let settings = demo::mailbox_settings(&upn);
                    (upn, settings)
                })
                .collect();
            store.mailbox_settings.extend(settings);
        }

        // Through the setter rather than the struct literal above, so the
        // sAMAccountName index is built with it — see `set_ad_users`.
        store.set_ad_users(demo::ad_users());

        let mut expanded = HashSet::new();
        expanded.insert("groups");
        expanded.insert("devices");
        expanded.insert("directory");

        Self {
            worker: None,
            phase: Phase::Ready,
            store,
            view: View::Overview,
            pane: Pane::Nav,
            expanded,
            nav_cursor: 0,
            views: HashMap::new(),
            account: Some("demo@contoso.co.uk".into()),
            tenant_label: "Contoso Demonstration (demo data)".into(),
            log_window: describe_log_window(7, 500),
            show_help: false,
            show_details: true,
            focus_filter: false,
            write_mode: WriteMode::Locked,
            writes_available: true,
            last_activity: Instant::now(),
            arming: false,
            pending: None,
            form: None,
            palette: None,
            import: None,
            // Demo mode has no worker, so the export is simulated rather than
            // attempted — the same arrangement as every other write here.
            mariadb: Some(demo::mariadb()),
            mariadb_password: None,
            database: None,
            // The demo domain is already "bound": there is no DC to reach, and
            // the point of the fixtures is to exercise the panes rather than
            // the dialog in front of them.
            directory: Some(demo::directory()),
            directory_password: Some(crate::mariadb::Secret::new("demo".into())),
            directory_prompt: None,
            directory_pending: None,
            database_progress: Vec::new(),
            batch: None,
            batch_failures: None,
            pending_update: None,
            last_action: None,
            status: "Demo data — no tenant connected".into(),
        }
    }

    /// Construct the console in a failed state, for when the config file could
    /// not be read. A GUI application should explain itself in a window rather
    /// than exit to a terminal nobody is watching.
    pub fn config_error(cc: &eframe::CreationContext<'_>, message: String) -> Self {
        theme::apply(&cc.egui_ctx);
        Self {
            worker: None,
            phase: Phase::Fatal(message),
            store: Store::default(),
            view: View::Overview,
            pane: Pane::Nav,
            expanded: HashSet::new(),
            nav_cursor: 0,
            views: HashMap::new(),
            account: None,
            tenant_label: String::new(),
            log_window: String::new(),
            show_help: false,
            show_details: true,
            focus_filter: false,
            write_mode: WriteMode::Locked,
            writes_available: true,
            last_activity: Instant::now(),
            arming: false,
            pending: None,
            form: None,
            palette: None,
            import: None,
            mariadb: None,
            mariadb_password: None,
            database: None,
            directory: None,
            directory_password: None,
            directory_prompt: None,
            directory_pending: None,
            database_progress: Vec::new(),
            batch: None,
            batch_failures: None,
            pending_update: None,
            last_action: None,
            status: "Configuration required".into(),
        }
    }

    pub fn view_state(&mut self, view: View) -> &mut ViewState {
        self.views.entry(view).or_default()
    }

    /// What the log views cover, for the caption beside their item count.
    pub fn log_window_caption(&self) -> &str {
        &self.log_window
    }

    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            worker.send(command);
        }
    }

    /// Dispatch a confirmed action.
    ///
    /// With no worker attached — demo mode — the write is simulated against the
    /// local store instead, so the confirmation flow, the audit trail and the
    /// resulting UI change can all be exercised without a tenant.
    fn dispatch(&mut self, action: Action) {
        if self.worker.is_some() {
            self.send(Command::Execute(Box::new(action)));
            return;
        }

        #[cfg(debug_assertions)]
        {
            let result = self.simulate(&action);
            self.handle_event(Event::ActionResult {
                label: action.label(),
                result,
            });
        }
    }

    /// Dispatch a confirmed batch.
    fn dispatch_many(&mut self, actions: Vec<Action>) {
        if actions.len() == 1 {
            self.dispatch(actions.into_iter().next().expect("length checked"));
            return;
        }

        if self.worker.is_some() {
            self.batch = Some(BatchProgress {
                done: 0,
                total: actions.len(),
            });
            self.send(Command::ExecuteBatch(actions));
            return;
        }

        #[cfg(debug_assertions)]
        {
            let total = actions.len();
            let mut failures = Vec::new();
            let mut succeeded = 0;
            for action in &actions {
                match self.simulate(action) {
                    Ok(()) => succeeded += 1,
                    Err(message) => failures.push((action.label(), message)),
                }
            }
            self.handle_event(Event::BatchDone {
                succeeded,
                failures,
            });
            let _ = total;
        }
    }

    /// Apply an action to the in-memory store, for demo mode only.
    #[cfg(debug_assertions)]
    fn simulate(&mut self, action: &Action) -> Result<(), String> {
        crate::actionlog::record_attempt(action, self.account.as_deref());

        let result = self.apply_to_store(action);
        if result.is_ok() {
            self.store.version += 1;
        }

        crate::actionlog::record_outcome(action, self.account.as_deref(), &result);
        result
    }

    /// Mutate the demo store the way Graph would mutate the tenant.
    #[cfg(debug_assertions)]
    fn apply_to_store(&mut self, action: &Action) -> Result<(), String> {
        use crate::graph::actions::MemberRole;

        match action {
            Action::CreateUser { spec } => {
                let users = Arc::make_mut(&mut self.store.users);
                if users
                    .iter()
                    .any(|user| user.upn().eq_ignore_ascii_case(&spec.user_principal_name))
                {
                    return Err("a user with that sign-in name already exists".into());
                }
                users.push(User {
                    id: format!("user-{:04}", users.len() + 100),
                    display_name: Some(spec.display_name.clone()),
                    user_principal_name: Some(spec.user_principal_name.clone()),
                    mail: Some(spec.user_principal_name.clone()),
                    job_title: spec.job_title.clone(),
                    department: spec.department.clone(),
                    account_enabled: Some(spec.account_enabled),
                    user_type: Some("Member".into()),
                    created_date_time: Some(chrono::Utc::now()),
                    usage_location: spec.usage_location.clone(),
                    ..Default::default()
                });
                users.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
                Ok(())
            }

            Action::Team { id, op, .. } => {
                use crate::graph::actions::TeamOp;
                let Some(Fetch::Ready(teams)) = &mut self.store.teams else {
                    return Err("teams are not loaded".into());
                };
                let teams = Arc::make_mut(teams);
                match op {
                    TeamOp::Delete => {
                        let before = teams.len();
                        teams.retain(|team| &team.id != id);
                        if teams.len() == before {
                            return Err("no such team".into());
                        }
                        self.store.team_details.remove(id);
                        Ok(())
                    }
                    TeamOp::Archive | TeamOp::Unarchive => {
                        let team = teams
                            .iter_mut()
                            .find(|team| &team.id == id)
                            .ok_or("no such team")?;
                        team.is_archived = Some(*op == TeamOp::Archive);
                        Ok(())
                    }
                }
            }

            Action::SetAutomaticReplies { name, spec, .. } => {
                // Keyed by UPN, as the real store is; the action carries an
                // object id, so the mailbox is found by name instead.
                let key = self
                    .store
                    .mailbox_settings
                    .keys()
                    .find(|upn| {
                        self.store
                            .mailboxes
                            .as_ref()
                            .and_then(|fetch| match fetch {
                                Fetch::Ready(mailboxes) => Some(mailboxes),
                                _ => None,
                            })
                            .is_some_and(|mailboxes| {
                                mailboxes.iter().any(|mailbox| {
                                    &mailbox.user_principal_name == *upn
                                        && mailbox.name() == name
                                })
                            })
                    })
                    .cloned()
                    .ok_or("no such mailbox")?;

                let Some(Fetch::Ready(settings)) = self.store.mailbox_settings.get_mut(&key)
                else {
                    return Err("those mailbox settings are not readable".into());
                };
                settings.automatic_replies_setting = Some(AutomaticReplies {
                    status: Some(if spec.enabled { "alwaysEnabled" } else { "disabled" }.into()),
                    external_audience: Some(spec.external_audience.clone()),
                    internal_reply_message: Some(spec.internal_message.clone()),
                    external_reply_message: Some(spec.external_message.clone()),
                    ..Default::default()
                });
                Ok(())
            }

            Action::SetUserEnabled { id, enabled, .. } => {
                let users = Arc::make_mut(&mut self.store.users);
                let user = users
                    .iter_mut()
                    .find(|user| &user.id == id)
                    .ok_or("no such user")?;
                user.account_enabled = Some(*enabled);
                Ok(())
            }

            Action::UpdateUser { id, patch, .. } => {
                let users = Arc::make_mut(&mut self.store.users);
                let user = users
                    .iter_mut()
                    .find(|user| &user.id == id)
                    .ok_or("no such user")?;
                // Blank clears the field, matching what Graph does.
                let set = |target: &mut Option<String>, value: &Option<String>| {
                    if let Some(value) = value {
                        *target = (!value.trim().is_empty()).then(|| value.clone());
                    }
                };
                set(&mut user.job_title, &patch.job_title);
                set(&mut user.department, &patch.department);
                set(&mut user.office_location, &patch.office_location);
                set(&mut user.mobile_phone, &patch.mobile_phone);
                set(&mut user.usage_location, &patch.usage_location);
                Ok(())
            }

            Action::SetLicense {
                id, sku_id, assign, ..
            } => {
                let users = Arc::make_mut(&mut self.store.users);
                let user = users
                    .iter_mut()
                    .find(|user| &user.id == id)
                    .ok_or("no such user")?;
                if *assign {
                    user.assigned_licenses.push(AssignedLicense {
                        sku_id: Some(sku_id.clone()),
                        disabled_plans: vec![],
                    });
                } else {
                    user.assigned_licenses
                        .retain(|licence| licence.sku_id.as_deref() != Some(sku_id.as_str()));
                }

                // Keep the seat count honest so the Licenses view agrees.
                let skus = Arc::make_mut(&mut self.store.licenses);
                if let Some(sku) = skus
                    .iter_mut()
                    .find(|sku| sku.sku_id.as_deref() == Some(sku_id.as_str()))
                {
                    let consumed = sku.consumed_units.unwrap_or(0);
                    sku.consumed_units =
                        Some(if *assign { consumed + 1 } else { (consumed - 1).max(0) });
                }
                Ok(())
            }

            Action::DeleteUser { id, .. } => {
                let users = Arc::make_mut(&mut self.store.users);
                let before = users.len();
                users.retain(|user| &user.id != id);
                if users.len() == before {
                    return Err("no such user".into());
                }
                Ok(())
            }

            Action::CreateGroup { spec } => {
                let groups = Arc::make_mut(&mut self.store.groups);
                groups.push(Group {
                    id: format!("group-{:04}", groups.len() + 100),
                    display_name: Some(spec.display_name.clone()),
                    description: spec.description.clone(),
                    mail_nickname: Some(spec.mail_nickname.clone()),
                    mail_enabled: Some(spec.unified),
                    security_enabled: Some(!spec.unified),
                    group_types: if spec.unified {
                        vec!["Unified".into()]
                    } else {
                        vec![]
                    },
                    created_date_time: Some(chrono::Utc::now()),
                    ..Default::default()
                });
                groups.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
                Ok(())
            }

            Action::UpdateGroup { id, patch, .. } => {
                let groups = Arc::make_mut(&mut self.store.groups);
                let group = groups
                    .iter_mut()
                    .find(|group| &group.id == id)
                    .ok_or("no such group")?;
                if let Some(name) = &patch.display_name {
                    group.display_name = Some(name.clone());
                }
                if let Some(description) = &patch.description {
                    group.description =
                        (!description.trim().is_empty()).then(|| description.clone());
                }
                Ok(())
            }

            Action::DeleteGroup { id, .. } => {
                let groups = Arc::make_mut(&mut self.store.groups);
                let before = groups.len();
                groups.retain(|group| &group.id != id);
                if groups.len() == before {
                    return Err("no such group".into());
                }
                self.store.group_members.remove(id);
                Ok(())
            }

            Action::SetMembership {
                group_id,
                member_id,
                member_name,
                role,
                add,
                ..
            } => {
                let (members, owners) = self
                    .store
                    .group_members
                    .entry(group_id.clone())
                    .or_insert_with(|| (Arc::new(Vec::new()), Arc::new(Vec::new())));
                let list = match role {
                    MemberRole::Member => Arc::make_mut(members),
                    MemberRole::Owner => Arc::make_mut(owners),
                };
                if *add {
                    list.push(DirectoryMember {
                        id: member_id.clone(),
                        display_name: Some(member_name.clone()),
                        user_principal_name: None,
                        odata_type: Some("#microsoft.graph.user".into()),
                    });
                } else {
                    list.retain(|member| &member.id != member_id);
                }
                Ok(())
            }

            Action::SetDeviceEnabled { id, enabled, .. } => {
                let devices = Arc::make_mut(&mut self.store.devices);
                let device = devices
                    .iter_mut()
                    .find(|device| &device.id == id)
                    .ok_or("no such device")?;
                device.account_enabled = Some(*enabled);
                Ok(())
            }

            Action::DeleteDevice { id, .. } => {
                let devices = Arc::make_mut(&mut self.store.devices);
                let before = devices.len();
                devices.retain(|device| &device.id != id);
                if devices.len() == before {
                    return Err("no such device".into());
                }
                Ok(())
            }

            // Passwords have nothing to change locally; report success so the
            // dialog flow can still be exercised.
            Action::ResetPassword { .. } => Ok(()),

            other => Err(format!("{} is not simulated in demo mode", other.label())),
        }
    }

    fn drain_events(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        for event in worker.drain() {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::SigningInSilently => {
                self.phase = Phase::SigningInSilently;
                self.status = "Signing in…".into();
            }
            Event::AwaitingBrowser { url } => {
                self.status = "Waiting for your browser".into();
                self.phase = Phase::AwaitingBrowser { url };
            }
            Event::SignedIn {
                account,
                writes_available,
            } => {
                self.account = account;
                self.writes_available = writes_available;
                self.phase = Phase::Ready;
                self.status = if writes_available {
                    "Loading directory…".into()
                } else {
                    "Read-only — the tenant did not grant the write permissions".into()
                };
            }
            Event::SignedOut => {
                self.account = None;
                self.store = Store::default();
                // Session state goes with the session. Somebody signing out to
                // hand the machine over should not leave a database password
                // behind them.
                self.mariadb_password = None;
                self.directory_password = None;
                self.directory_prompt = None;
                self.directory_pending = None;
                self.phase = Phase::SigningInSilently;
                self.status = "Signing in…".into();
                self.send(Command::SignIn);
            }
            Event::Loading(collection) => {
                self.store.loading.insert(collection);
                self.store.errors.remove(&collection);
            }
            Event::Organization(org) => {
                self.tenant_label = org.name().to_string();
                self.store.org = Some(*org);
            }
            Event::Users(users) => {
                self.store.set_users(users);
                self.finish(Collection::Users);
            }
            Event::Groups(groups) => {
                self.store.groups = groups;
                self.finish(Collection::Groups);
            }
            Event::Roles(roles) => {
                self.store.roles = roles;
                self.finish(Collection::Roles);
            }
            Event::Devices(devices) => {
                self.store.devices = devices;
                self.finish(Collection::Devices);
            }
            Event::ManagedDevices(fetch) => {
                self.store.managed = Some(fetch);
                self.finish(Collection::ManagedDevices);
            }
            Event::Licenses(licenses) => {
                self.store.licenses = licenses;
                self.finish(Collection::Licenses);
            }
            Event::SignIns(fetch) => {
                self.store.sign_ins = Some(fetch);
                self.finish(Collection::SignIns);
            }
            Event::AuditLogs(fetch) => {
                self.store.audits = Some(fetch);
                self.finish(Collection::AuditLogs);
            }
            Event::Teams(fetch) => {
                self.store.teams = Some(fetch);
                self.finish(Collection::Teams);
            }
            Event::AdUsers(fetch) => {
                self.store.set_ad_users(fetch);
                self.finish(Collection::AdUsers);
            }
            Event::AdComputers(fetch) => {
                self.store.ad_computers = Some(fetch);
                self.finish(Collection::AdComputers);
            }
            Event::DirectoryBound => {
                self.directory_prompt = None;
                self.status = "Connected to Active Directory".into();
                // The bind was only ever a credential check; the collection
                // that wanted it still has to be read.
                if let Some(collection) = self.directory_pending.take() {
                    // Integrated authentication carries no password; a simple
                    // bind has just proved one. Either is a reason to read,
                    // but "neither" is not.
                    let password = self.directory_password.clone();
                    if password.is_some() || self.directory_is_integrated() {
                        self.send(Command::LoadDirectory {
                            collection,
                            password,
                        });
                    }
                }
            }
            Event::DirectoryBindFailed(message) => {
                self.directory_password = None;
                self.status = "Could not connect to Active Directory".into();
                if let Some(prompt) = &mut self.directory_prompt {
                    prompt.failed(message);
                } else {
                    // The dialog was dismissed while the bind was in flight;
                    // the reason still belongs somewhere the operator can see.
                    self.last_action = Some(Err(message));
                }
            }
            Event::Mailboxes(fetch) => {
                self.store.set_mailboxes(fetch);
                self.finish(Collection::Mailboxes);
            }
            Event::TeamDetail {
                team_id,
                team,
                channels,
            } => {
                self.store.team_details.insert(team_id, (*team, channels));
            }
            Event::MailboxSettings { key, settings } => {
                self.store.mailbox_settings.insert(key, *settings);
            }
            Event::GroupMembers {
                group_id,
                members,
                owners,
            } => {
                self.store.loading.remove(&Collection::Groups);
                self.store.group_members.insert(group_id, (members, owners));
            }
            Event::RoleMembers { role_id, members } => {
                self.store.role_members.insert(role_id, members);
            }
            Event::UserMemberships {
                user_id,
                memberships,
            } => {
                self.store.user_memberships.insert(user_id, memberships);
            }
            Event::Failed {
                collection,
                message,
            } => match collection {
                Some(collection) => {
                    self.store.loading.remove(&collection);
                    self.store.errors.insert(collection, message);
                    self.status = format!("{} could not be loaded", collection.label());
                }
                None => self.store.notice = Some(message),
            },
            Event::Fatal(message) => {
                self.phase = Phase::Fatal(message);
            }
            Event::WriteMode(armed) => {
                // Mirror what the worker actually believes, not what we asked
                // for — the worker is the authority on whether writes can run.
                self.write_mode = if armed {
                    WriteMode::Armed
                } else {
                    WriteMode::Locked
                };
            }
            Event::ActionResult { label, result } => {
                self.last_action = Some(match &result {
                    Ok(()) => Ok(label.clone()),
                    Err(message) => Err(format!("{label} — {message}")),
                });
                self.status = match &result {
                    Ok(()) => format!("{label} — done"),
                    Err(_) => format!("{label} — failed"),
                };
                // A completed write invalidates cached membership details.
                self.store.requested.clear();
            }
            Event::BatchProgress { done, total } => {
                self.batch = Some(BatchProgress { done, total });
                self.status = format!("Applying {done} of {total}…");
            }
            Event::BatchDone {
                succeeded,
                failures,
            } => {
                self.batch = None;
                self.store.requested.clear();
                self.status = if failures.is_empty() {
                    format!("{succeeded} applied")
                } else {
                    format!("{succeeded} applied, {} failed", failures.len())
                };
                if !failures.is_empty() {
                    self.batch_failures = Some((succeeded, failures));
                }
            }
            Event::DatabaseProgress { table, rows } => {
                self.database_progress.push((table.clone(), rows));
                self.status = format!("Wrote {table} ({rows} rows)…");
            }
            Event::DatabaseDone(result) => {
                match &result {
                    Ok(summary) => {
                        self.status = format!("Exported {summary}");
                        self.last_action = Some(Ok(format!("Exported {summary}")));
                    }
                    Err(message) => {
                        self.status = "Database export failed".into();
                        self.last_action =
                            Some(Err(format!("Database export failed — {message}")));
                        // A refused connection is very often a wrong password,
                        // and keeping it would mean every retry this session
                        // failed the same way without ever asking again.
                        if looks_like_a_credential_problem(message) {
                            self.mariadb_password = None;
                        }
                    }
                }
                self.database_progress.clear();
            }
            Event::UpdateAvailable(release) => {
                self.pending_update = Some(*release);
            }
            Event::UpdateApplyFailed(message) => {
                self.status = "Update failed".into();
                self.last_action = Some(Err(format!(
                    "Update failed — {message}. gcm is unchanged; try again later or \
                     download it by hand from the release page."
                )));
            }
            Event::WriteRejected(label) => {
                // Only reachable if something bypassed the UI gate; say so
                // plainly rather than failing silently.
                self.last_action = Some(Err(format!(
                    "{label} was refused because write mode is not armed"
                )));
                self.status = "Write refused — write mode is off".into();
            }
        }
    }

    /// Route an action through confirmation, or straight to the worker when it
    /// is harmless.
    ///
    /// Every action in the UI goes through here; nothing calls
    /// `Command::Execute` directly.
    fn request_action(&mut self, action: Action) {
        self.request_actions(vec![action]);
    }

    /// Route one or more actions through confirmation, or straight to the
    /// worker when every one of them is harmless.
    fn request_actions(&mut self, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        if !self.write_mode.is_armed() {
            self.status = "Write mode is off — press Ctrl+Shift+W to enable it".into();
            return;
        }

        self.touch();

        let all_safe = actions.iter().all(|a| a.severity() == Severity::Safe);
        if all_safe && actions.len() == 1 {
            self.dispatch(actions.into_iter().next().expect("length checked"));
        } else {
            self.pending = Some(confirm::Pending::new(actions));
        }
    }

    /// Put one on-premises change through the same gate and the same
    /// confirmation as a tenant change.
    ///
    /// Not merged with [`Self::request_actions`] because the two carry
    /// different action types, but the sequence is deliberately identical —
    /// gate, then confirm unless it is safe, then dispatch. An on-premises
    /// write that skipped either step would be a hole in the guarantee the
    /// tenant side is careful about.
    fn request_directory_action(&mut self, action: crate::ldap::actions::DirectoryAction) {
        if !self.write_mode.is_armed() {
            self.status = "Write mode is off — press Ctrl+Shift+W to enable it".into();
            return;
        }

        self.touch();

        if action.severity() == Severity::Safe {
            self.dispatch_directory(action);
        } else {
            self.pending = Some(confirm::Pending::for_directory(action));
        }
    }

    /// Send a confirmed on-premises change to the worker.
    ///
    /// `refresh` is the view being looked at, because a DN does not say
    /// whether it belongs to a user or a computer and the wrong pane would
    /// otherwise be re-read.
    fn dispatch_directory(&mut self, action: crate::ldap::actions::DirectoryAction) {
        let refresh = self
            .view
            .collection()
            .filter(|collection| collection.is_directory())
            .unwrap_or(Collection::AdUsers);

        if self.worker.is_some() {
            self.send(Command::DirectoryAction {
                action: Box::new(action),
                password: self.directory_password.clone(),
                refresh,
            });
            return;
        }

        // Demo mode has no domain controller to write to, and pretending
        // otherwise would make the AD panes look writable when they are not.
        #[cfg(debug_assertions)]
        self.handle_event(Event::ActionResult {
            label: action.label(),
            result: Err(
                "Demo mode has no domain controller, so on-premises changes are not \
                 simulated."
                    .into(),
            ),
        });
    }

    /// Note that the operator did something, deferring the idle timeout.
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    fn set_write_mode(&mut self, armed: bool) {
        if armed && !self.writes_available {
            self.status =
                "Write mode is unavailable — the tenant did not grant the write permissions"
                    .into();
            return;
        }

        self.write_mode = if armed {
            WriteMode::Armed
        } else {
            WriteMode::Locked
        };
        self.send(Command::SetWriteMode(armed));
        self.touch();
        self.status = if armed {
            "Write mode enabled".into()
        } else {
            "Write mode off".into()
        };
    }

    /// Disarm after a spell of inactivity.
    fn expire_write_mode(&mut self, ctx: &egui::Context) {
        if !self.write_mode.is_armed() {
            return;
        }

        // Any real input counts as activity. `events` covers keys, clicks and
        // scrolling; pointer motion alone deliberately does not, so a cat on
        // the desk cannot hold write mode open.
        if ctx.input(|i| !i.events.is_empty()) {
            self.touch();
            return;
        }

        if self.last_activity.elapsed() >= WRITE_IDLE_TIMEOUT {
            self.set_write_mode(false);
            self.status = "Write mode turned off after 15 minutes idle".into();
            self.pending = None;
        } else {
            // Make sure a frame lands near the deadline even if nothing else
            // asks for one, or an idle console would never notice.
            ctx.request_repaint_after(Duration::from_secs(30));
        }
    }

    fn finish(&mut self, collection: Collection) {
        self.store.loading.remove(&collection);
        self.store.errors.remove(&collection);
        self.store.version += 1;
        self.status = if self.store.loading.is_empty() {
            "Ready".into()
        } else {
            format!("Loading {}…", self.store.loading.len())
        };
    }

    /// Ask the worker for details of the selected object, once per object.
    ///
    /// The key is prefixed by view because two collections can legitimately
    /// share an id — a team and the group behind it have the same one, and
    /// without the prefix selecting the team would be silently satisfied by the
    /// group's already-requested membership.
    fn ensure_details(&mut self) {
        let request = match self.view {
            View::Users => self.selected_user().map(|user| {
                (
                    format!("user:{}", user.id),
                    Command::UserMemberships {
                        user_id: user.id.clone(),
                    },
                )
            }),
            View::Groups => self.selected_group().map(|group| {
                (
                    format!("group:{}", group.id),
                    Command::GroupMembers {
                        group_id: group.id.clone(),
                    },
                )
            }),
            View::Roles => self.selected_role().map(|role| {
                (
                    format!("role:{}", role.id),
                    Command::RoleMembers {
                        role_id: role.id.clone(),
                    },
                )
            }),
            View::Teams => self.selected_team().map(|team| {
                (
                    format!("team:{}", team.id),
                    Command::TeamDetail {
                        team_id: team.id.clone(),
                    },
                )
            }),
            View::Mailboxes => self.selected_mailbox().map(|mailbox| {
                let key = Self::mailbox_settings_key(mailbox).to_string();
                // Prefer the directory object id where the account is loaded:
                // Graph accepts either, and an object id cannot be tripped up
                // by a UPN the report anonymised.
                let lookup = self
                    .store
                    .users
                    .iter()
                    .find(|user| user.upn().eq_ignore_ascii_case(&key))
                    .map(|user| user.id.clone())
                    .unwrap_or_else(|| key.clone());
                (
                    format!("mailbox:{key}"),
                    Command::MailboxSettings { lookup, key },
                )
            }),
            _ => None,
        };

        if let Some((key, command)) = request
            && self.store.requested.insert(key)
        {
            self.send(command);
        }
    }

    fn selected_user(&self) -> Option<&User> {
        let state = self.views.get(&View::Users)?;
        self.store.users.get(state.selected_source()?)
    }

    fn selected_ad_user(&self) -> Option<&AdUser> {
        let state = self.views.get(&View::AdUsers)?;
        match &self.store.ad_users {
            Some(Fetch::Ready(users)) => users.get(state.selected_source()?),
            _ => None,
        }
    }

    fn selected_ad_computer(&self) -> Option<&AdComputer> {
        let state = self.views.get(&View::AdComputers)?;
        match &self.store.ad_computers {
            Some(Fetch::Ready(computers)) => computers.get(state.selected_source()?),
            _ => None,
        }
    }

    fn selected_group(&self) -> Option<&Group> {
        let state = self.views.get(&View::Groups)?;
        self.store.groups.get(state.selected_source()?)
    }

    fn selected_role(&self) -> Option<&DirectoryRole> {
        let state = self.views.get(&View::Roles)?;
        self.store.roles.get(state.selected_source()?)
    }

    fn selected_team(&self) -> Option<&Team> {
        let state = self.views.get(&View::Teams)?;
        match &self.store.teams {
            Some(Fetch::Ready(teams)) => teams.get(state.selected_source()?),
            _ => None,
        }
    }

    fn selected_mailbox(&self) -> Option<&Mailbox> {
        let state = self.views.get(&View::Mailboxes)?;
        self.store.mailbox_rows.get(state.selected_source()?)
    }

    /// The identifier the mailbox settings cache is keyed by for a mailbox.
    fn mailbox_settings_key(mailbox: &Mailbox) -> &str {
        &mailbox.user_principal_name
    }

    /// Rebuild the filtered index list for the current view.
    fn refresh_filter(&mut self) {
        let version = self.store.version;
        let view = self.view;
        // Cloned handles keep the borrow checker happy while the closure below
        // reads the data and the state is mutated.
        match view {
            View::Overview => {}
            View::Users => {
                let data = self.store.users.clone();
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::user_matches(&data[i], needle));
            }
            View::Groups => {
                let data = self.store.groups.clone();
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::group_matches(&data[i], needle));
            }
            View::Roles => {
                let data = self.store.roles.clone();
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::role_matches(&data[i], needle));
            }
            View::Devices => {
                let data = self.store.devices.clone();
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::device_matches(&data[i], needle));
            }
            View::ManagedDevices => {
                let data = Store::optional(&self.store.managed);
                let len = data.len();
                self.view_state(view).refresh(version, len, |i, needle| {
                    list::managed_matches(&data[i], needle)
                });
            }
            View::Licenses => {
                let data = self.store.licenses.clone();
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::sku_matches(&data[i], needle));
            }
            View::Mailboxes => {
                let data = self.store.mailbox_rows.clone();
                let len = data.len();
                self.view_state(view).refresh(version, len, |i, needle| {
                    list::mailbox_matches(&data[i], needle)
                });
            }
            View::Teams => {
                let data = Store::optional(&self.store.teams);
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::team_matches(&data[i], needle));
            }
            View::SignIns => {
                let data = Store::optional(&self.store.sign_ins);
                let len = data.len();
                self.view_state(view).refresh(version, len, |i, needle| {
                    list::sign_in_matches(&data[i], needle)
                });
            }
            View::AuditLogs => {
                let data = Store::optional(&self.store.audits);
                let len = data.len();
                self.view_state(view)
                    .refresh(version, len, |i, needle| list::audit_matches(&data[i], needle));
            }
            View::AdUsers => {
                let data = Store::optional(&self.store.ad_users);
                let len = data.len();
                self.view_state(view).refresh(version, len, |i, needle| {
                    list::ad_user_matches(&data[i], needle)
                });
            }
            View::AdComputers => {
                let data = Store::optional(&self.store.ad_computers);
                let len = data.len();
                self.view_state(view).refresh(version, len, |i, needle| {
                    list::ad_computer_matches(&data[i], needle)
                });
            }
        }
    }

    fn go_to(&mut self, view: View) {
        self.view = view;
        self.nav_cursor = nav::index_of(&self.expanded, view).unwrap_or(self.nav_cursor);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();

        let ctx = ui.ctx().clone();


        if let Phase::Fatal(message) = &self.phase {
            let message = message.clone();
            self.fatal_screen(ui, &message);
            return;
        }

        if !matches!(self.phase, Phase::Ready) {
            let url = match &self.phase {
                Phase::AwaitingBrowser { url } => Some(url.clone()),
                _ => None,
            };
            self.sign_in_screen(ui, url);
            return;
        }

        // Modals own the keyboard while they are up, so shortcuts are skipped
        // rather than firing behind a confirmation dialog.
        let modal_open = self.pending.is_some()
            || self.arming
            || self.form.is_some()
            || self.palette.is_some()
            || self.import.is_some()
            || self.database.is_some()
            || self.directory_prompt.is_some()
            || self.batch_failures.is_some();
        if !modal_open {
            keys::handle(self, &ctx);
        }
        self.expire_write_mode(&ctx);
        self.refresh_filter();
        self.ensure_details();
        self.ensure_directory_loaded();

        self.top_bar(ui);
        self.status_bar(ui);

        egui::Panel::left("scope")
            .resizable(true)
            .default_size(232.0)
            .size_range(180.0..=340.0)
            .show(ui, |ui| nav::show(self, ui));

        if self.show_details {
            egui::Panel::right("details")
                .resizable(true)
                .default_size(384.0)
                .size_range(260.0..=620.0)
                .show(ui, |ui| details::show(self, ui));
        }

        egui::CentralPanel::default().show(ui, |ui| list::show(self, ui));

        if self.show_help {
            help::show(&ctx, &mut self.show_help);
        }

        self.modals(&ctx);
    }
}

impl App {
    /// Open the actions menu for the current selection, or the ticked set.
    ///
    /// Unlike the buttons, this opens while read-only: seeing what is available
    /// should not require arming write mode first. Entries are simply disabled.
    fn open_palette(&mut self) {
        let view = self.view;
        let Some(state) = self.views.get(&view) else {
            return;
        };

        let marked: Vec<usize> = state.marked.iter().copied().collect();
        let palette = if marked.len() > 1 {
            let bulk = menu::bulk_for(self, view, &marked);
            menu::Palette::for_bulk(bulk, marked.len())
        } else {
            let Some(source) = state.selected_source() else {
                return;
            };
            let items = menu::for_object(self, view, source);
            let subject = list::row_name(self, view, source);
            menu::Palette::for_single(items, subject)
        };

        if palette.is_empty() {
            self.status = quips::nothing_to_do(view.title());
            return;
        }

        self.palette = Some(palette);
    }

    /// Run one of the console's own commands — the entries that need no write
    /// mode and, until now, had only a keyboard shortcut.
    ///
    /// Each arm does exactly what the corresponding key does, by calling the
    /// same method, so the menu cannot come to mean something different from
    /// the accelerator printed beside it.
    fn run_view_command(&mut self, ctx: &egui::Context, command: menu::ViewCommand) {
        let view = self.view;
        match command {
            menu::ViewCommand::CopyRow => {
                let text = details::copy_text(self);
                if !text.is_empty() {
                    ctx.copy_text(text);
                    self.status = "Copied to clipboard".into();
                }
            }
            menu::ViewCommand::ToggleMark => self.view_state(view).toggle_mark(),
            menu::ViewCommand::MarkAllFiltered => {
                self.view_state(view).mark_all_filtered()
            }
            menu::ViewCommand::ClearMarks => self.view_state(view).clear_marks(),
            menu::ViewCommand::ExportCsv => self.export(export::Format::Csv),
            menu::ViewCommand::ExportJson => self.export(export::Format::Json),
            menu::ViewCommand::Refresh => self.refresh_current(),
        }
    }

    /// Open the create-user form, and move to the Users node so the new account
    /// is visible in the list the moment it is created.
    fn new_user(&mut self) {
        self.open_form(forms::Form::create_user());
        if self.form.is_some() {
            self.go_to(View::Users);
        }
    }

    /// Open the create-group form, and move to the Groups node so the new
    /// group is visible in the list the moment it is created.
    fn new_group(&mut self) {
        self.open_form(forms::Form::CreateGroup {
            display_name: String::new(),
            mail_nickname: String::new(),
            description: String::new(),
            unified: false,
        });
        if self.form.is_some() {
            self.go_to(View::Groups);
        }
    }

    /// `Ctrl+N` and the toolbar button share this: a group while looking at
    /// the Groups pane, a user everywhere else.
    fn new_user_or_group(&mut self) {
        match self.view {
            View::Groups => self.new_group(),
            View::Users => self.new_user(),
            // Ctrl+N from a node where nothing can be created used to open the
            // create-user form and jump to Users, which is a surprising place
            // to end up from the Licenses pane.
            other => {
                self.status = format!(
                    "Nothing can be created from {} — go to Users or Groups",
                    other.title()
                )
            }
        }
    }

    /// Open a form, refusing while read-only so the gate is one rule, not two.
    fn open_form(&mut self, form: forms::Form) {
        if !self.write_mode.is_armed() {
            self.status = "Write mode is off — press Ctrl+Shift+W to enable it".into();
            return;
        }
        self.touch();
        self.form = Some(form);
    }

    /// Report what a partially-failed batch actually did.
    ///
    /// Graph writes are not transactional, so a run that fails at item seven
    /// leaves six applied. Naming them is the difference between a recoverable
    /// situation and one reconstructed by hand.
    fn failure_modal(&mut self, ctx: &egui::Context) {
        let Some((succeeded, failures)) = self.batch_failures.clone() else {
            return;
        };

        let mut dismiss = false;
        let response = egui::Modal::new(egui::Id::new("batch-failures")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(
                RichText::new("Some changes did not apply")
                    .size(15.0)
                    .strong()
                    .color(theme::WARN),
            );
            ui.add_space(10.0);
            ui.label(format!(
                "{succeeded} succeeded, {} failed. Those that succeeded have \
                 already taken effect.",
                failures.len()
            ));
            ui.add_space(12.0);

            egui::ScrollArea::vertical()
                .max_height(280.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (label, message) in &failures {
                        ui.label(RichText::new(label).strong().color(theme::BAD));
                        ui.label(RichText::new(message).small().color(theme::MUTED));
                        ui.add_space(8.0);
                    }
                });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Copy report").clicked() {
                    let report = failures
                        .iter()
                        .map(|(label, message)| format!("{label}: {message}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(report);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        dismiss = true;
                    }
                });
            });

            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                dismiss = true;
            }
        });

        if dismiss || response.should_close() {
            self.batch_failures = None;
        }
    }

    /// Draw whichever confirmation is outstanding, and act on the answer.
    fn modals(&mut self, ctx: &egui::Context) {
        // A failure report outranks everything: it is the only place the
        // operator learns what a partial batch actually changed.
        if self.batch_failures.is_some() {
            self.failure_modal(ctx);
            return;
        }

        if let Some(mut prompt) = self.database.take() {
            // Cloned rather than borrowed: the dialog needs `&mut self` for the
            // outcome, and the settings live on `self`.
            let Some(settings) = self.mariadb.clone() else {
                return;
            };
            match database::show(ctx, &mut prompt, &settings, self.mariadb_password.as_ref()) {
                database::Outcome::Export(password) => {
                    self.mariadb_password = Some(password.clone());
                    self.database_progress.clear();
                    self.status = format!("Exporting to {}…", settings.describe());

                    if self.worker.is_some() {
                        self.send(Command::ExportToDatabase {
                            password,
                            tables: prompt.tables,
                        });
                    } else {
                        #[cfg(debug_assertions)]
                        self.simulate_database_export(&settings, prompt.tables);
                    }
                }
                database::Outcome::Cancelled => self.status = "Export cancelled".into(),
                database::Outcome::Pending => self.database = Some(prompt),
            }
            return;
        }

        if let Some(mut prompt) = self.directory_prompt.take() {
            // Cloned for the same reason as the export settings above: the
            // dialog needs `&mut self` for its outcome.
            let Some(settings) = self.directory.clone() else {
                return;
            };
            match directory::show(ctx, &mut prompt, &settings) {
                directory::Outcome::Bind(password) => {
                    self.status = format!("Connecting to {}…", settings.host);
                    // Held provisionally, and dropped again if the DC refuses
                    // it, so a rejected password is never left behind for the
                    // next view to retry with.
                    self.directory_password = Some(password.clone());
                    self.send(Command::VerifyDirectoryBind {
                        password: Some(password),
                    });
                    // Kept open, showing "Connecting…", until the worker either
                    // confirms the bind or says why it was refused.
                    self.directory_prompt = Some(prompt);
                }
                directory::Outcome::Cancelled => {
                    // Say so against the view itself. Without this the list
                    // renders its ordinary "No items." — which is a false
                    // statement about a domain that was never read, and the
                    // dialog does not reopen on its own, because doing that
                    // would make cancelling impossible. Recording it as an
                    // error also means F5 clears it and asks again.
                    if let Some(collection) = self.directory_pending.take() {
                        self.store.errors.insert(
                            collection,
                            format!(
                                "Not connected to {}. Refresh this node (F5) to enter \
                                 the bind password.",
                                settings.host
                            ),
                        );
                    }
                    self.status = "Not connected to Active Directory".into();
                }
                directory::Outcome::Pending => self.directory_prompt = Some(prompt),
            }
            return;
        }

        if let Some(plan) = self.import.take() {
            match import::show(ctx, &plan, self.write_mode.is_armed()) {
                import::Outcome::Apply => self.request_actions(plan.actions),
                import::Outcome::Cancelled => self.status = "Import cancelled".into(),
                import::Outcome::Pending => self.import = Some(plan),
            }
            return;
        }

        if let Some(mut palette) = self.palette.take() {
            match menu::palette(ctx, &mut palette, self.write_mode.is_armed()) {
                menu::Chosen::Act(actions) => self.request_actions(actions),
                menu::Chosen::ActDirectory(action) => self.request_directory_action(*action),
                menu::Chosen::Open(form) => self.open_form(form.build()),
                menu::Chosen::View(command) => self.run_view_command(ctx, command),
                menu::Chosen::Cancelled => {}
                menu::Chosen::Pending => self.palette = Some(palette),
            }
            return;
        }

        // Forms run first: submitting one produces an action that then needs
        // confirming, so the two modals hand over within a single frame.
        if let Some(mut form) = self.form.take() {
            match forms::show(ctx, &mut form, &self.store) {
                forms::Outcome::Submit(action) => {
                    self.request_action(*action);
                }
                forms::Outcome::SubmitDirectory(action) => {
                    self.request_directory_action(*action);
                }
                forms::Outcome::Cancelled => {}
                forms::Outcome::Pending => self.form = Some(form),
            }
            return;
        }

        if self.arming {
            match confirm::arm_modal(ctx) {
                confirm::Outcome::Confirmed => {
                    self.arming = false;
                    self.set_write_mode(true);
                }
                confirm::Outcome::Cancelled => self.arming = false,
                confirm::Outcome::Pending => {}
            }
            return;
        }

        let Some(pending) = &mut self.pending else {
            // Lowest priority of everything above: an available update is
            // worth mentioning, never worth interrupting a form, a
            // confirmation, or anything else already on screen for.
            if let Some(release) = self.pending_update.take() {
                match update::show(ctx, &release) {
                    update::Outcome::Update => {
                        self.status = format!("Downloading gcm {}…", release.version);
                        self.send(Command::ApplyUpdate(Box::new(release)));
                    }
                    update::Outcome::OpenReleasePage => {
                        let _ = open::that_detached(&release.html_url);
                        self.status = "Opened the release page in your browser".into();
                    }
                    update::Outcome::Dismissed => {}
                    update::Outcome::Pending => self.pending_update = Some(release),
                }
            }
            return;
        };

        match confirm::action_modal(ctx, pending) {
            confirm::Outcome::Confirmed => {
                let subject = self.pending.take().expect("pending was just borrowed").subject;
                // Re-check the gate at the moment of execution: write mode can
                // expire while a confirmation sits open.
                if !self.write_mode.is_armed() {
                    self.status = "Write mode expired before that was confirmed".into();
                    return;
                }
                self.touch();
                match subject {
                    confirm::Subject::Tenant(actions) => self.dispatch_many(actions),
                    confirm::Subject::Directory(action) => self.dispatch_directory(*action),
                }
            }
            confirm::Outcome::Cancelled => {
                self.pending = None;
                self.status = "Cancelled".into();
            }
            confirm::Outcome::Pending => {}
        }
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(FRIENDLY_NAME).strong());
                ui.separator();

                if ui
                    .button("Refresh")
                    .on_hover_text("Reload the current view (F5)")
                    .clicked()
                {
                    self.refresh_current();
                }
                if ui
                    .button("Refresh all")
                    .on_hover_text("Reload every collection (Ctrl+Shift+R)")
                    .clicked()
                {
                    self.store.requested.clear();
                    self.send(Command::LoadAll);
                }

                let details_label = if self.show_details {
                    "Hide details"
                } else {
                    "Show details"
                };
                if ui
                    .button(details_label)
                    .on_hover_text("Toggle the details pane (Ctrl+D)")
                    .clicked()
                {
                    self.show_details = !self.show_details;
                }

                ui.separator();

                // Follows the node rather than sitting there offering "New
                // user…" from the Licenses pane. Only two things in the console
                // can be created, so on the other nine nodes the button is
                // simply absent — a button that names the wrong object is worse
                // than no button, because it reads as the one thing this pane
                // can do.
                // One slot, filled by whatever this node's primary action is,
                // rather than a button per possibility. "New user…" used to sit
                // here on all eleven nodes, naming the wrong object on nine of
                // them; now it appears only where something can actually be
                // created, and the read-only nodes get the thing that *is*
                // useful there instead. One slot also keeps the row from
                // growing wide enough to collide with the account controls
                // pinned to the right.
                match menu::creatable(self.view) {
                    Some((new_label, new_disabled_hover)) => {
                        if ui
                            .add_enabled(
                                self.write_mode.is_armed(),
                                egui::Button::new(new_label),
                            )
                            .on_hover_text(format!(
                                "{} (Ctrl+N)",
                                new_label.trim_end_matches('…')
                            ))
                            .on_disabled_hover_text(new_disabled_hover)
                            .clicked()
                        {
                            self.new_user_or_group();
                        }
                    }
                    None if self.view != View::Overview => {
                        let exportable =
                            self.store.count(self.view).is_some_and(|rows| rows > 0);
                        if ui
                            .add_enabled(exportable, egui::Button::new("Export…"))
                            .on_hover_text(
                                "Export this view (Ctrl+E for CSV, Ctrl+Shift+E for JSON)",
                            )
                            .on_disabled_hover_text(
                                "There is nothing loaded in this view to export",
                            )
                            .clicked()
                        {
                            self.export(export::Format::Csv);
                        }
                    }
                    None => {}
                }

                // Only when there is somewhere to export to. A button that
                // exists solely to say "not configured" is worse than no button.
                if self.mariadb.is_some() {
                    ui.separator();
                    if ui
                        .button("Export to MariaDB")
                        .on_hover_text(
                            "Replace the database tables with every loaded view \
                             (Ctrl+Shift+D)",
                        )
                        .clicked()
                    {
                        self.export_to_database();
                    }
                }

                ui.separator();

                if ui
                    .button("Keyboard help")
                    .on_hover_text("Show all shortcuts (F1)")
                    .clicked()
                {
                    self.show_help = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Sign out")
                        .on_hover_text("Forget the cached credential and sign in again")
                        .clicked()
                    {
                        self.send(Command::SignOut);
                    }
                    if let Some(account) = &self.account {
                        ui.label(RichText::new(account).color(theme::MUTED));
                    }

                    ui.separator();

                    // The single most important thing on screen when armed.
                    if self.write_mode.is_armed() {
                        let chip = egui::Button::new(
                            RichText::new("● WRITE ENABLED").color(Color32::WHITE).strong(),
                        )
                        .fill(theme::BAD);
                        if ui
                            .add(chip)
                            .on_hover_text(
                                "This console can change your tenant. Click, or press \
                                 Ctrl+Shift+W, to return to read-only.",
                            )
                            .clicked()
                        {
                            self.set_write_mode(false);
                        }
                    } else if self.writes_available {
                        if ui
                            .button("Read-only")
                            .on_hover_text("Enable write mode (Ctrl+Shift+W)")
                            .clicked()
                        {
                            self.arming = true;
                        }
                    } else {
                        ui.label(RichText::new("Read-only").color(theme::MUTED))
                            .on_hover_text(
                                "The tenant did not grant the write permissions at \
                                 sign-in, so this session cannot change anything. \
                                 Grant admin consent on the app registration and \
                                 sign out to retry.",
                            );
                    }
                });
            });
            self.object_bar(ui);
            ui.add_space(3.0);
        });
    }

    /// A second toolbar row carrying what can be done to the selected object.
    ///
    /// Drawn from [`menu::for_object`], the same source as the right-click menu
    /// and the details-pane button bar, so a console with three ways to reach an
    /// action cannot offer three different sets of them.
    ///
    /// Absent entirely when there is nothing to show — on a view whose objects
    /// Graph will not let anyone change, or with no row selected — rather than
    /// an empty strip of chrome. It appears and disappears with the selection,
    /// which is the price of not lying about what is available.
    fn object_bar(&mut self, ui: &mut egui::Ui) {
        let view = self.view;
        let Some(source) = self.views.get(&view).and_then(|s| s.selected_source()) else {
            return;
        };

        // Bulk takes precedence: with rows ticked, the toolbar should act on
        // the ticked set, exactly as the right-click menu does.
        let marked: Vec<usize> = self
            .views
            .get(&view)
            .map(|state| state.marked.iter().copied().collect())
            .unwrap_or_default();

        let armed = self.write_mode.is_armed();

        if marked.len() > 1 {
            let bulk = menu::bulk_for(self, view, &marked);
            if bulk.is_empty() {
                return;
            }
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} selected", marked.len()))
                        .small()
                        .color(theme::MUTED),
                );
                ui.separator();
                for (label, actions) in bulk {
                    let destructive = actions
                        .iter()
                        .any(|action| action.severity() == Severity::Destructive);
                    let text = if destructive {
                        RichText::new(&label)
                            .color(if armed { theme::BAD } else { theme::MUTED })
                    } else {
                        RichText::new(&label)
                    };
                    if ui
                        .add_enabled(armed, egui::Button::new(text))
                        .on_disabled_hover_text(
                            "Enable write mode (Ctrl+Shift+W) to use this",
                        )
                        .clicked()
                    {
                        self.request_actions(actions);
                    }
                }
            });
            return;
        }

        // Creating an object is not something you do *to* the selected one,
        // and the slot above already offers it — without this the toolbar
        // showed "New user…" twice, one line apart.
        let items: Vec<menu::Item> = menu::for_object(self, view, source)
            .into_iter()
            .filter(|item| !item.creates_object())
            .collect();
        if items.iter().all(|item| matches!(item, menu::Item::Separator)) {
            return;
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(list::row_name(self, view, source))
                    .small()
                    .color(theme::MUTED),
            );
            ui.separator();
            for item in items {
                match item {
                    menu::Item::Separator => {}
                    menu::Item::ActDirectory { label, action } => {
                        let destructive = action.severity() == Severity::Destructive;
                        let text = if destructive {
                            RichText::new(&label)
                                .color(if armed { theme::BAD } else { theme::MUTED })
                        } else {
                            RichText::new(&label)
                        };
                        if ui
                            .add_enabled(armed, egui::Button::new(text))
                            .on_disabled_hover_text(
                                "Enable write mode (Ctrl+Shift+W) to use this",
                            )
                            .clicked()
                        {
                            self.request_directory_action(*action);
                        }
                    }
                    menu::Item::Act { label, action } => {
                        let destructive = action.severity() == Severity::Destructive;
                        let text = if destructive {
                            RichText::new(&label)
                                .color(if armed { theme::BAD } else { theme::MUTED })
                        } else {
                            RichText::new(&label)
                        };
                        if ui
                            .add_enabled(armed, egui::Button::new(text))
                            .on_disabled_hover_text(
                                "Enable write mode (Ctrl+Shift+W) to use this",
                            )
                            .clicked()
                        {
                            self.request_action(action);
                        }
                    }
                    menu::Item::Open { label, form } => {
                        if ui
                            .add_enabled(armed, egui::Button::new(&label))
                            .on_disabled_hover_text(
                                "Enable write mode (Ctrl+Shift+W) to use this",
                            )
                            .clicked()
                        {
                            self.open_form(form.build());
                        }
                    }
                    // The console's own commands already have toolbar buttons
                    // of their own further up; `for_object` does not carry them
                    // anyway, and this arm exists only to stay exhaustive.
                    menu::Item::View { .. } => {}
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if !self.store.loading.is_empty() || self.batch.is_some() {
                    ui.spinner();
                }
                ui.label(RichText::new(&self.status).color(theme::MUTED));

                if let Some(batch) = self.batch {
                    ui.add(
                        egui::ProgressBar::new(
                            batch.done as f32 / batch.total.max(1) as f32,
                        )
                        .desired_width(140.0)
                        .text(format!("{} / {}", batch.done, batch.total)),
                    );
                }

                if let Some(notice) = self.store.notice.clone() {
                    ui.separator();
                    ui.label(RichText::new(notice).color(theme::WARN));
                }

                // A failed write must not scroll away with the next status
                // update, so it stays until the following action.
                if let Some(Err(failure)) = &self.last_action {
                    ui.separator();
                    ui.label(RichText::new(failure).color(theme::BAD));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("F6 changes pane · F1 for shortcuts")
                            .color(theme::MUTED),
                    );
                    ui.separator();
                    if self.write_mode.is_armed() {
                        ui.label(RichText::new("WRITE").color(theme::BAD).strong());
                        ui.separator();
                    }
                    let focus = match self.pane {
                        Pane::Nav => "Scope",
                        Pane::List => "Results",
                        Pane::Details => "Details",
                    };
                    ui.label(RichText::new(format!("Focus: {focus}")).color(theme::MUTED));
                    ui.separator();
                    if !self.tenant_label.is_empty() {
                        ui.label(RichText::new(&self.tenant_label).color(theme::MUTED));
                    }
                });
            });
            ui.add_space(2.0);
        });
    }

    /// Report a database export as though it had run, for demo mode.
    ///
    /// Nothing is written anywhere. This exists so the dialog, the progress in
    /// the status bar and the completion message can be seen without a database
    /// to point at — the same reason `simulate` exists for tenant writes.
    #[cfg(debug_assertions)]
    fn simulate_database_export(
        &mut self,
        settings: &crate::config::MariaDb,
        tables: Vec<crate::mariadb::Table>,
    ) {
        let total: usize = tables.iter().map(|table| table.rows.len()).sum();
        let count = tables.len();
        let names: Vec<String> = tables
            .iter()
            .map(|table| settings.table_for(table.stem))
            .collect();

        for (name, table) in names.iter().zip(&tables) {
            self.handle_event(Event::DatabaseProgress {
                table: name.clone(),
                rows: table.rows.len(),
            });
        }
        self.handle_event(Event::DatabaseDone(Ok(format!(
            "{total} rows into {count} tables ({}) at {} — simulated, nothing was written",
            names.join(", "),
            settings.describe()
        ))));
    }

    /// Open the database export dialog, or explain why there is none.
    ///
    /// Unlike a tenant write, this does not require write mode: it changes
    /// nothing in Microsoft 365. The gate it does have is the dialog, which
    /// names every table it is about to replace.
    fn export_to_database(&mut self) {
        let Some(settings) = self.mariadb.clone() else {
            self.status = format!(
                "No [mariadb] section in {} — add one to enable this",
                source_label()
            );
            return;
        };

        let tables = export::database_tables(self);
        if tables.is_empty() {
            self.status = "Nothing has finished loading yet, so there is nothing to export"
                .into();
            return;
        }

        let remembered = self.mariadb_password.is_some();
        self.database = Some(database::Prompt::new(tables, remembered));
        let _ = settings;
    }

    /// Read a CSV and show what it would do. Nothing runs until approved.
    fn open_import(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Comma-separated values", &["csv"])
            .pick_file()
        else {
            return;
        };

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                self.last_action =
                    Some(Err(format!("Could not read {}: {err}", path.display())));
                return;
            }
        };

        let source = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        let directory = crate::importer::Directory {
            users: &self.store.users,
            groups: &self.store.groups,
            licences: &self.store.licenses,
        };

        match crate::importer::plan(&text, source, directory) {
            Ok(plan) => {
                self.status = format!(
                    "{} to apply, {} skipped",
                    plan.actions.len(),
                    plan.skipped.len()
                );
                self.import = Some(plan);
            }
            Err(err) => {
                self.last_action = Some(Err(format!("Import failed: {err:#}")));
                self.status = "Could not read that file".into();
            }
        }
    }

    /// Write the current view to a file the operator chooses.
    fn export(&mut self, format: export::Format) {
        if self.view == View::Overview {
            self.status = "There is nothing to export from the console root".into();
            return;
        }
        match export::save(self, self.view, format) {
            Ok(Some(path)) => {
                self.status = format!("Exported to {}", path.display());
            }
            // Cancelling the dialog is not a failure.
            Ok(None) => {}
            Err(err) => {
                self.last_action = Some(Err(format!("Export failed: {err:#}")));
                self.status = "Export failed".into();
            }
        }
    }

    pub fn refresh_current(&mut self) {
        self.refresh_view(self.view);
    }

    /// Refresh a specific node, regardless of which one is currently
    /// selected — what the nav tree's context menu acts on.
    fn refresh_view(&mut self, view: View) {
        let Some(collection) = view.collection() else {
            self.send(Command::LoadAll);
            return;
        };

        self.store.requested.clear();

        // An on-premises collection cannot be read from Graph, and cannot be
        // read at all without the bind password. Clearing the request marker
        // lets `ensure_directory_loaded` do both — re-reading when the password
        // is already held, and re-opening the dialog when it is not.
        if collection.is_directory() {
            self.store.errors.remove(&collection);
            // Only the collection being refreshed. Dropping both would discard
            // a good computer list to re-read the users, and would leave the
            // Users pane briefly unable to explain itself.
            match collection {
                Collection::AdComputers => self.store.ad_computers = None,
                _ => self.store.clear_ad_users(),
            }
            return;
        }

        self.send(Command::Load(collection));
    }

    /// Read the selected on-premises collection, asking to bind first if
    /// nothing has been bound yet.
    ///
    /// Lazy, unlike every Graph collection, which `LoadAll` fetches up front.
    /// Two reasons, and both are about the credential rather than the data:
    /// prompting for a bind password at launch would demand it from every
    /// operator who never opens the node, and the worker cannot fetch these on
    /// its own because it deliberately does not keep the password.
    fn ensure_directory_loaded(&mut self) {
        if !self.view.is_directory() {
            return;
        }
        let Some(collection) = self.view.collection() else {
            return;
        };

        // Loaded, in flight, already explained as unavailable, or failed once
        // already — a failure must not become a retry loop against a DC.
        if self.store.count(self.view).is_some()
            || self.store.loading.contains(&collection)
            || self.store.unavailable(self.view).is_some()
            || self.store.errors.contains_key(&collection)
            || self.directory_prompt.is_some()
        {
            return;
        }

        // No [directory] section: the worker has already said so, and saying it
        // again here would only race with that.
        if self.directory.is_none() {
            return;
        }

        // Requested but not yet acknowledged. Without this the next frame would
        // send the same read again, because `Event::Loading` has not arrived.
        let key = format!("directory:{}", collection.label());
        if !self.store.requested.insert(key) {
            return;
        }

        // Integrated authentication has nothing to ask for: the read binds as
        // the signed-in Windows account, so the dialog would be a prompt for a
        // credential that is not used. Straight to the read.
        if self.directory_is_integrated() {
            self.send(Command::LoadDirectory {
                collection,
                password: None,
            });
            return;
        }

        match self.directory_password.clone() {
            Some(password) => self.send(Command::LoadDirectory {
                collection,
                password: Some(password),
            }),
            None => {
                self.directory_pending = Some(collection);
                self.directory_prompt = Some(directory::Prompt::new());
            }
        }
    }

    /// Whether the domain controller is reached as the signed-in Windows
    /// account, and so needs no password collected for it.
    fn directory_is_integrated(&self) -> bool {
        self.directory
            .as_ref()
            .is_some_and(crate::config::Directory::uses_integrated_auth)
    }

    fn sign_in_screen(&mut self, ui: &mut egui::Ui, url: Option<String>) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(90.0);
                ui.heading(FRIENDLY_NAME);
                ui.add_space(6.0);
                ui.label(RichText::new("Microsoft 365 management console").color(theme::MUTED));
                ui.add_space(32.0);

                match url {
                    None => {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label("Signing in with your saved credential…");
                    }
                    Some(url) => {
                        ui.label(
                            RichText::new("Finish signing in in your browser")
                                .size(15.0)
                                .strong(),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(
                                "A browser tab should have opened. Sign in there and \
                                 this window will continue on its own.",
                            )
                            .color(theme::MUTED),
                        );
                        ui.add_space(20.0);
                        ui.spinner();
                        ui.add_space(20.0);

                        ui.horizontal(|ui| {
                            // Centre the pair under the message.
                            ui.add_space((ui.available_width() - 300.0).max(0.0) / 2.0);
                            if ui.button("Open the sign-in page again").clicked() {
                                let _ = open::that_detached(&url);
                            }
                            if ui.button("Copy the link").clicked() {
                                ui.ctx().copy_text(url.clone());
                            }
                        });

                        ui.add_space(24.0);
                        ui.label(
                            RichText::new(
                                "Signing in through your own browser lets Entra see this \
                                 device, so Conditional Access policies can evaluate it.",
                            )
                            .small()
                            .color(theme::MUTED),
                        );
                    }
                }
            });
        });
    }

    fn fatal_screen(&mut self, ui: &mut egui::Ui, message: &str) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.heading(FRIENDLY_NAME);
                ui.add_space(20.0);
                ui.label(RichText::new("Cannot continue").size(17.0).color(theme::BAD));
            });
            ui.add_space(16.0);

            egui::Frame::group(ui.style())
                .fill(Color32::WHITE)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(message).monospace());
                });

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                // Nothing to open on Windows: the configuration lives in the
                // registry, not in this folder, which holds only the logs
                // "Open the error log" below already reaches. Offering it
                // there would send somebody looking for config.ini into a
                // folder that has never had one.
                #[cfg(not(windows))]
                if ui.button("Open the configuration folder").clicked() {
                    let _ = open::that_detached(crate::config::config_dir());
                }
                // A file path is worth copying because somebody can open it.
                // A registry key path is not — so on Windows this hands over
                // the configuration itself, rendered back into the documented
                // file format. That is what the file backend's "safe to paste
                // into a support ticket" property becomes when the settings
                // move into the registry; neither form contains a password.
                #[cfg(windows)]
                if ui
                    .button("Copy the configuration")
                    .on_hover_text(format!(
                        "Copies the contents of {} as INI text. It contains no passwords.",
                        crate::config::source_label()
                    ))
                    .clicked()
                {
                    ui.ctx().copy_text(crate::registry::export_ini());
                }
                #[cfg(not(windows))]
                if ui.button("Copy the file path").clicked() {
                    ui.ctx()
                        .copy_text(crate::config::config_path().display().to_string());
                }
                if ui
                    .button("Open the error log")
                    .on_hover_text(crate::errorlog::log_path().display().to_string())
                    .clicked()
                {
                    let _ = open::that_detached(crate::errorlog::log_path());
                }
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "Diagnostics are written to {}",
                    crate::errorlog::log_path().display()
                ))
                .small()
                .color(theme::MUTED),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entra_user(sam: Option<&str>) -> User {
        User {
            id: "user-0001".into(),
            display_name: Some("Aisha Rahman".into()),
            user_principal_name: Some("aisha.rahman@contoso.co.uk".into()),
            on_premises_sync_enabled: Some(sam.is_some()),
            on_premises_sam_account_name: sam.map(String::from),
            ..Default::default()
        }
    }

    fn ad_user(sam: &str) -> AdUser {
        AdUser {
            dn: "CN=Aisha Rahman,OU=Finance,DC=corp,DC=contoso,DC=co,DC=uk".into(),
            sam_account_name: Some(sam.into()),
            ..Default::default()
        }
    }

    fn store_with(users: Vec<AdUser>) -> Store {
        let mut store = Store::default();
        store.set_ad_users(Fetch::Ready(Arc::new(users)));
        store
    }

    #[test]
    fn a_synced_account_is_joined_to_its_ad_object() {
        let store = store_with(vec![ad_user("someone.else"), ad_user("aisha.rahman")]);
        let matched = store
            .ad_match(&entra_user(Some("aisha.rahman")))
            .expect("the synced account must find its AD object");
        assert_eq!(matched.ou(), "corp.contoso.co.uk/Finance");
    }

    #[test]
    fn the_join_folds_case_on_both_sides() {
        // AD treats sAMAccountName case-insensitively, and Entra echoes back
        // whatever case was set at sync time. Matching exactly would drop the
        // join for any account whose two directories disagree about capitals.
        let store = store_with(vec![ad_user("Aisha.Rahman")]);
        assert!(
            store.ad_match(&entra_user(Some("aisha.rahman"))).is_some(),
            "a case difference must not break the join"
        );
    }

    #[test]
    fn a_cloud_only_account_has_no_on_premises_half() {
        let store = store_with(vec![ad_user("aisha.rahman")]);
        assert!(store.ad_match(&entra_user(None)).is_none());
    }

    #[test]
    fn an_account_outside_the_base_dn_simply_does_not_match() {
        // Synced, but its AD object is not under the configured base — a real
        // situation in a multi-domain forest, and not an error.
        let store = store_with(vec![ad_user("someone.else")]);
        assert!(store.ad_match(&entra_user(Some("aisha.rahman"))).is_none());
    }

    #[test]
    fn nothing_matches_before_the_directory_has_been_read() {
        let store = Store::default();
        assert!(store.ad_match(&entra_user(Some("aisha.rahman"))).is_none());
    }

    #[test]
    fn an_unavailable_directory_clears_a_previous_index() {
        // Otherwise a stale index would go on resolving to indices in a list
        // that is no longer there — `ad_match` would return the wrong account
        // or, worse, a plausible one.
        let mut store = store_with(vec![ad_user("aisha.rahman")]);
        assert!(store.ad_match(&entra_user(Some("aisha.rahman"))).is_some());

        store.set_ad_users(Fetch::Unavailable("no domain".into()));
        assert!(store.ad_match(&entra_user(Some("aisha.rahman"))).is_none());
    }

    #[test]
    fn the_index_survives_a_reload_that_reorders_the_list() {
        // The index holds positions, so it must be rebuilt with the list it
        // describes rather than carried over from the previous read.
        let mut store = store_with(vec![ad_user("aisha.rahman"), ad_user("svc-sql")]);
        store.set_ad_users(Fetch::Ready(Arc::new(vec![
            ad_user("svc-sql"),
            ad_user("aisha.rahman"),
        ])));

        let matched = store
            .ad_match(&entra_user(Some("aisha.rahman")))
            .expect("still joined after a reload");
        assert_eq!(matched.sam(), "aisha.rahman");
    }

    /// The demo fixtures have to agree with each other, or the console shows
    /// "no matching account was found" for every synthetic user and the join
    /// looks broken when only the fixtures were.
    #[cfg(debug_assertions)]
    #[test]
    fn the_demo_fixtures_actually_correlate() {
        let mut store = Store::default();
        store.set_ad_users(crate::demo::ad_users());

        let synced: Vec<User> = crate::demo::users()
            .iter()
            .filter(|user| user.on_premises_sync_enabled.unwrap_or(false))
            .cloned()
            .collect();
        assert!(!synced.is_empty(), "the fixtures must include synced users");

        let matched = synced
            .iter()
            .filter(|user| store.ad_match(user).is_some())
            .count();
        assert!(
            matched > 0,
            "no demo user joined to an AD object — the two fixtures disagree about \
             sAMAccountName"
        );
    }

    // ---- The mailbox union -------------------------------------------------

    fn reported(upn: &str, storage: i64) -> Mailbox {
        Mailbox {
            user_principal_name: upn.into(),
            display_name: upn.split('@').next().unwrap_or(upn).into(),
            storage_used: storage,
            prohibit_send_receive_quota: 100,
            source: MailboxSource::Report,
            ..Default::default()
        }
    }

    fn mailbox_user(upn: &str, has_mailbox: bool) -> User {
        User {
            id: format!("id-{upn}"),
            display_name: Some(upn.split('@').next().unwrap_or(upn).into()),
            user_principal_name: Some(upn.into()),
            assigned_plans: if has_mailbox {
                vec![AssignedPlan {
                    service: Some("exchange".into()),
                    capability_status: Some("Enabled".into()),
                    ..Default::default()
                }]
            } else {
                vec![]
            },
            ..Default::default()
        }
    }

    fn union_of(report: Vec<Mailbox>, users: Vec<User>) -> Arc<Vec<Mailbox>> {
        let mut store = Store::default();
        store.set_users(Arc::new(users));
        store.set_mailboxes(Fetch::Ready(Arc::new(report)));
        store.mailbox_rows.clone()
    }

    #[test]
    fn a_mailbox_the_report_has_not_reached_still_appears() {
        // The whole point. The report is compiled daily, so a mailbox created
        // this morning is in the directory and nowhere else.
        let rows = union_of(
            vec![reported("aisha@contoso.co.uk", 50)],
            vec![
                mailbox_user("aisha@contoso.co.uk", true),
                mailbox_user("new.starter@contoso.co.uk", true),
            ],
        );

        assert_eq!(rows.len(), 2);
        let new = rows
            .iter()
            .find(|mailbox| mailbox.upn() == "new.starter@contoso.co.uk")
            .expect("the new mailbox must be listed");
        assert!(!new.metrics_known(), "it has no figures yet");
        assert_eq!(new.storage_display(), "Not yet reported");
        assert_eq!(new.item_count_display(), "—");
    }

    #[test]
    fn an_account_without_an_exchange_plan_is_not_a_mailbox() {
        let rows = union_of(
            vec![],
            vec![
                mailbox_user("no.mailbox@contoso.co.uk", false),
                mailbox_user("has.one@contoso.co.uk", true),
            ],
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].upn(), "has.one@contoso.co.uk");
    }

    #[test]
    fn the_report_wins_where_both_sources_know_the_mailbox() {
        // Otherwise every licensed mailbox would appear twice, once with
        // figures and once without.
        let rows = union_of(
            vec![reported("aisha@contoso.co.uk", 50)],
            vec![mailbox_user("aisha@contoso.co.uk", true)],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].metrics_known(), "the reported row must survive");
        assert_eq!(rows[0].storage_used, 50);
    }

    #[test]
    fn the_join_folds_case_on_both_sides_of_the_upn() {
        // Entra echoes back whatever case the UPN was created with; the report
        // has its own ideas. Matching literally would double every mailbox
        // whose case differs between the two.
        let rows = union_of(
            vec![reported("Aisha.Rahman@Contoso.co.uk", 50)],
            vec![mailbox_user("aisha.rahman@contoso.co.uk", true)],
        );
        assert_eq!(rows.len(), 1, "got {rows:#?}");
    }

    #[test]
    fn unreported_mailboxes_sort_below_every_reported_one() {
        // A row with no figures must not land among the empty mailboxes, where
        // a nil usage fraction would otherwise put it.
        let rows = union_of(
            vec![
                reported("full@contoso.co.uk", 99),
                reported("empty@contoso.co.uk", 0),
            ],
            vec![mailbox_user("brand.new@contoso.co.uk", true)],
        );

        let names: Vec<&str> = rows.iter().map(Mailbox::upn).collect();
        assert_eq!(
            names,
            ["full@contoso.co.uk", "empty@contoso.co.uk", "brand.new@contoso.co.uk"]
        );
    }

    #[test]
    fn a_concealed_report_is_left_alone_rather_than_joined() {
        // With report identities anonymised there is no key to match on, so a
        // join would match nothing and append every mailbox a second time
        // under its real name.
        let mut concealed = reported("a1b2c3d4-0000-0000-0000-000000000000", 50);
        concealed.display_name = "A1B2C3D4".into();
        assert!(concealed.is_concealed());

        let rows = union_of(
            vec![concealed],
            vec![mailbox_user("aisha@contoso.co.uk", true)],
        );
        assert_eq!(rows.len(), 1, "the directory half must be skipped entirely");
    }

    #[test]
    fn no_report_means_no_list_rather_than_a_list_without_figures() {
        // A tenant with no Exchange, or an app registration without
        // Reports.Read.All, gets the view's own explanation. Half a list with
        // every column blank would be a worse answer than a clear one.
        let mut store = Store::default();
        store.set_users(Arc::new(vec![mailbox_user("aisha@contoso.co.uk", true)]));
        store.set_mailboxes(Fetch::Unavailable("no Exchange here".into()));

        assert!(store.mailbox_rows.is_empty());
        assert_eq!(store.count(View::Mailboxes), None);
    }

    #[test]
    fn the_demo_fixtures_exercise_both_halves_of_the_union() {
        let mut store = Store::default();
        store.set_users(crate::demo::users());
        store.set_mailboxes(crate::demo::mailboxes());

        let reported = store
            .mailbox_rows
            .iter()
            .filter(|mailbox| mailbox.metrics_known())
            .count();
        let from_directory = store.mailbox_rows.len() - reported;

        assert!(reported > 0, "the demo must show reported mailboxes");
        assert!(
            from_directory > 0,
            "the demo must also show a mailbox the report has not reached, or the \
             union is never visible while developing"
        );
    }

    #[test]
    fn on_premises_collections_are_routed_away_from_graph() {
        // `refresh_view` sends `Command::Load` for Graph collections and must
        // not for these — the worker would reject it, and the view would sit
        // there looking broken.
        assert!(Collection::AdUsers.is_directory());
        assert!(Collection::AdComputers.is_directory());
        assert!(!Collection::Users.is_directory());
        assert!(View::AdUsers.is_directory());
        assert!(!View::Users.is_directory());
    }

    #[test]
    fn every_on_premises_view_has_a_distinct_export_table() {
        // The AD views join `View::ALL`, so they are exported alongside the
        // tenant's own collections; a stem collision would silently overwrite
        // somebody's table.
        let mut stems: Vec<&str> = View::ALL.iter().filter_map(|v| v.table_stem()).collect();
        let total = stems.len();
        stems.sort_unstable();
        stems.dedup();
        assert_eq!(stems.len(), total, "two views share an export table name");
        assert!(stems.contains(&"ad_users"));
        assert!(stems.contains(&"ad_computers"));
    }
}

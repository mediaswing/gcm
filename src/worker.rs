//! The background half of the application.
//!
//! egui repaints on the main thread and must never block, so every Graph call
//! happens on a Tokio runtime owned by this module. The two halves talk over a
//! pair of channels: [`Command`] going out, [`Event`] coming back. The UI holds
//! no locks and awaits nothing.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use anyhow::Result;
use tokio::sync::mpsc as tokio_mpsc;

use crate::actionlog;
use crate::auth::{AuthProgress, Authenticator};
use crate::errorlog;
use crate::config::Config;
use crate::graph::actions::Action;
use crate::graph::models::*;
use crate::graph::{Fetch, GraphClient};

/// Which collection a load request refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Collection {
    Users,
    Groups,
    Roles,
    Devices,
    ManagedDevices,
    Licenses,
    SignIns,
    AuditLogs,
    Teams,
    Mailboxes,
}

impl Collection {
    pub fn label(self) -> &'static str {
        match self {
            Collection::Users => "Users",
            Collection::Groups => "Groups",
            Collection::Roles => "Directory roles",
            Collection::Devices => "Entra devices",
            Collection::ManagedDevices => "Managed devices",
            Collection::Licenses => "Licenses",
            Collection::SignIns => "Sign-in logs",
            Collection::AuditLogs => "Audit logs",
            Collection::Teams => "Teams",
            Collection::Mailboxes => "Mailboxes",
        }
    }
}

/// Requests from the UI to the worker.
#[derive(Debug, Clone)]
pub enum Command {
    SignIn,
    /// Load a collection, refreshing it if already present.
    Load(Collection),
    /// Load everything the console shows, in a sensible order.
    LoadAll,
    /// Fetch the members of a group, for the details pane.
    GroupMembers { group_id: String },
    /// Fetch the members of a directory role.
    RoleMembers { role_id: String },
    /// Fetch the groups and roles a user belongs to.
    UserMemberships { user_id: String },
    /// Fetch a team's full settings and its channels, for the details pane.
    /// `/teams` returns only four populated properties, so the rest has to be
    /// asked for one team at a time.
    TeamDetail { team_id: String },
    /// Fetch one mailbox's settings, for the details pane.
    ///
    /// `lookup` is what Graph is asked for — an object id where the account is
    /// loaded, otherwise the UPN, since Graph accepts either. `key` is what the
    /// answer is filed under, and is always the UPN, because that is the only
    /// identifier the mailbox usage report carries.
    MailboxSettings { lookup: String, key: String },
    /// Forget the cached refresh token and sign in again.
    SignOut,
    /// Arm or disarm write mode. The worker keeps its own copy of this and
    /// refuses every mutation while it is false.
    SetWriteMode(bool),
    /// Perform one mutation.
    Execute(Box<Action>),
    /// Perform many mutations, one after another.
    ExecuteBatch(Vec<Action>),
    /// Replace the configured MariaDB tables with the collections given.
    ///
    /// The password travels with the command rather than being held by the
    /// worker, so there is exactly one copy of it in the process and its
    /// lifetime is the UI's to manage.
    ExportToDatabase {
        password: crate::mariadb::Secret,
        tables: Vec<crate::mariadb::Table>,
    },
}

/// Results from the worker back to the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// Signing in silently with a cached refresh token.
    SigningInSilently,
    /// The browser has been opened for sign-in; we are waiting for it.
    AwaitingBrowser { url: String },
    SignedIn {
        account: Option<String>,
        /// False when the tenant refused the write scopes, so the console runs
        /// read-only for this session.
        writes_available: bool,
    },
    SignedOut,
    /// A collection began loading; the UI shows a spinner for it.
    Loading(Collection),
    Organization(Box<Organization>),
    Users(Arc<Vec<User>>),
    Groups(Arc<Vec<Group>>),
    Roles(Arc<Vec<DirectoryRole>>),
    Devices(Arc<Vec<Device>>),
    /// Managed devices, or the reason the tenant cannot supply them.
    ManagedDevices(Fetch<Arc<Vec<ManagedDevice>>>),
    Licenses(Arc<Vec<SubscribedSku>>),
    /// Recent sign-ins, or the reason the tenant cannot supply them.
    SignIns(Fetch<Arc<Vec<SignIn>>>),
    /// Recent directory changes, or the reason they are unavailable.
    AuditLogs(Fetch<Arc<Vec<DirectoryAudit>>>),
    Teams(Fetch<Arc<Vec<Team>>>),
    Mailboxes(Fetch<Arc<Vec<Mailbox>>>),
    /// A team's full settings and channel list.
    TeamDetail {
        team_id: String,
        team: Box<Fetch<Team>>,
        channels: Arc<Vec<Channel>>,
    },
    /// One mailbox's settings, keyed by the owner's UPN.
    MailboxSettings {
        key: String,
        settings: Box<Fetch<crate::graph::models::MailboxSettings>>,
    },
    GroupMembers {
        group_id: String,
        members: Arc<Vec<DirectoryMember>>,
        owners: Arc<Vec<DirectoryMember>>,
    },
    RoleMembers {
        role_id: String,
        members: Arc<Vec<DirectoryMember>>,
    },
    UserMemberships {
        user_id: String,
        memberships: Arc<Vec<DirectoryMember>>,
    },
    /// A collection failed to load. The UI shows this against that collection.
    Failed {
        collection: Option<Collection>,
        message: String,
    },
    /// A fatal problem — bad config, refused sign-in. The UI blocks on it.
    Fatal(String),
    /// Write mode changed, echoed back so the UI reflects what the worker
    /// actually believes rather than what it hoped.
    WriteMode(bool),
    /// A mutation finished.
    ActionResult {
        label: String,
        result: Result<(), String>,
    },
    /// A mutation arrived while write mode was locked. This should be
    /// unreachable through the UI; if it fires, something bypassed the gate.
    WriteRejected(String),
    /// Progress through a batch.
    BatchProgress { done: usize, total: usize },
    /// A batch finished. `failures` pairs the action label with its error.
    BatchDone {
        succeeded: usize,
        failures: Vec<(String, String)>,
    },
    /// One table of a database export landed.
    DatabaseProgress { table: String, rows: usize },
    /// A database export finished, successfully or otherwise.
    DatabaseDone(Result<String, String>),
}

/// The UI's handle on the worker thread.
pub struct Worker {
    commands: tokio_mpsc::UnboundedSender<Command>,
    events: Receiver<Event>,
}

impl Worker {
    /// Spawn the worker thread and its Tokio runtime.
    ///
    /// `repaint` is called whenever an event is queued, so egui wakes up even
    /// when the user is not touching the keyboard.
    pub fn spawn(config: Config, repaint: impl Fn() + Send + 'static) -> Result<Self> {
        let (command_tx, command_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = channel();

        thread::Builder::new()
            .name("gcm-graph".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = event_tx
                            .send(Event::Fatal(format!("could not start the runtime: {err}")));
                        repaint();
                        return;
                    }
                };
                runtime.block_on(run(config, command_rx, event_tx, repaint));
            })?;

        Ok(Self {
            commands: command_tx,
            events: event_rx,
        })
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drain everything queued since the last frame.
    pub fn drain(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}

/// Sends events to the UI and pokes egui to repaint.
struct Reporter<F: Fn()> {
    tx: Sender<Event>,
    repaint: F,
}

impl<F: Fn()> Reporter<F> {
    /// Send an event, recording anything that went wrong in the diagnostic log
    /// on the way past.
    ///
    /// Routing it through here rather than at each call site is what makes the
    /// log complete: every failure the UI is told about reaches this function
    /// by construction, so none can be added later and quietly go unrecorded.
    fn send(&self, event: Event) {
        match &event {
            Event::Fatal(message) => errorlog::error("worker", message),
            Event::Failed {
                collection,
                message,
            } => {
                let area = collection.map(Collection::label).unwrap_or("tenant");
                errorlog::error(area, message);
            }
            Event::WriteRejected(label) => errorlog::warn(
                "write-gate",
                &format!("refused {label} — write mode is not armed"),
            ),
            Event::ActionResult {
                label,
                result: Err(message),
            } => errorlog::error("action", &format!("{label} — {message}")),
            Event::BatchDone { failures, .. } if !failures.is_empty() => {
                for (label, message) in failures {
                    errorlog::error("batch", &format!("{label} — {message}"));
                }
            }
            // Safe to log: the connection is described without its password,
            // and `Secret` cannot be formatted into a message by accident.
            Event::DatabaseDone(Ok(summary)) => {
                errorlog::info("mariadb", &format!("export wrote {summary}"))
            }
            Event::DatabaseDone(Err(message)) => errorlog::error("mariadb", message),
            Event::SignedIn {
                account,
                writes_available,
            } => errorlog::info(
                "auth",
                &format!(
                    "signed in as {} (writes {})",
                    account.as_deref().unwrap_or("unknown"),
                    if *writes_available {
                        "available"
                    } else {
                        "refused"
                    }
                ),
            ),
            // A tenant that does not offer a feature is a normal state, but it
            // is also the single most common thing somebody opens this log to
            // understand — "why is this view empty?"
            Event::ManagedDevices(Fetch::Unavailable(reason)) => {
                errorlog::warn("managed-devices", reason)
            }
            Event::SignIns(Fetch::Unavailable(reason)) => errorlog::warn("sign-ins", reason),
            Event::AuditLogs(Fetch::Unavailable(reason)) => errorlog::warn("audit-logs", reason),
            Event::Teams(Fetch::Unavailable(reason)) => errorlog::warn("teams", reason),
            Event::Mailboxes(Fetch::Unavailable(reason)) => errorlog::warn("mailboxes", reason),
            _ => {}
        }

        let _ = self.tx.send(event);
        (self.repaint)();
    }

    /// Report a `Result`, converting the error into a per-collection failure.
    fn report<T>(&self, collection: Collection, result: Result<T>, on_ok: impl FnOnce(T) -> Event) {
        match result {
            Ok(value) => self.send(on_ok(value)),
            Err(err) => self.send(Event::Failed {
                collection: Some(collection),
                message: format!("{err:#}"),
            }),
        }
    }
}

async fn run<F: Fn() + Send + 'static>(
    config: Config,
    mut commands: tokio_mpsc::UnboundedReceiver<Command>,
    events: Sender<Event>,
    repaint: F,
) {
    let reporter = Reporter { tx: events, repaint };

    let http = match reqwest::Client::builder()
        .user_agent(concat!("gcm/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            reporter.send(Event::Fatal(format!("could not start the HTTP client: {err}")));
            return;
        }
    };

    let auth = Authenticator::new(config.clone(), http.clone());
    // Kept aside before the client takes ownership: the database export needs
    // the connection details but has nothing to do with Graph.
    let config_for_export = config.mariadb.clone();
    let mut client = GraphClient::new(config, http, auth);

    // Starts locked on every launch and is never persisted, so a console that
    // was armed yesterday opens read-only today.
    let mut write_armed = false;

    while let Some(command) = commands.recv().await {
        match command {
            Command::SignIn => {
                if sign_in(&mut client, &reporter).await {
                    load_all(&mut client, &reporter).await;
                }
            }
            Command::SignOut => {
                crate::auth::clear_cache();
                reporter.send(Event::SignedOut);
            }
            Command::LoadAll => load_all(&mut client, &reporter).await,
            Command::Load(collection) => load_one(&mut client, &reporter, collection).await,
            Command::GroupMembers { group_id } => {
                reporter.send(Event::Loading(Collection::Groups));
                let members = client.group_members(&group_id).await;
                let owners = client.group_owners(&group_id).await;
                match (members, owners) {
                    (Ok(members), owners) => reporter.send(Event::GroupMembers {
                        group_id,
                        members: Arc::new(members),
                        // Owner reads fail on some group types; an empty owner
                        // list is a better answer than losing the members too.
                        owners: Arc::new(owners.unwrap_or_default()),
                    }),
                    (Err(err), _) => reporter.send(Event::Failed {
                        collection: Some(Collection::Groups),
                        message: format!("{err:#}"),
                    }),
                }
            }
            Command::RoleMembers { role_id } => {
                let result = client.role_members(&role_id).await;
                reporter.report(Collection::Roles, result, |members| Event::RoleMembers {
                    role_id,
                    members: Arc::new(members),
                });
            }
            Command::UserMemberships { user_id } => {
                let result = client.user_memberships(&user_id).await;
                reporter.report(Collection::Users, result, |memberships| {
                    Event::UserMemberships {
                        user_id,
                        memberships: Arc::new(memberships),
                    }
                });
            }
            Command::TeamDetail { team_id } => {
                let team = client.team(&team_id).await;
                // Channels are a separate read and a separate permission, so a
                // team whose settings are refused can still list its channels
                // and vice versa. Losing both because one failed would be worse
                // than showing whichever half arrived.
                let channels = client.team_channels(&team_id).await;
                match team {
                    Ok(team) => reporter.send(Event::TeamDetail {
                        team_id,
                        team: Box::new(team),
                        channels: Arc::new(channels.unwrap_or_default()),
                    }),
                    Err(err) => reporter.send(Event::Failed {
                        collection: Some(Collection::Teams),
                        message: format!("{err:#}"),
                    }),
                }
            }
            Command::MailboxSettings { lookup, key } => {
                let result = client.mailbox_settings(&lookup).await;
                reporter.report(Collection::Mailboxes, result, |settings| {
                    Event::MailboxSettings {
                        key,
                        settings: Box::new(settings),
                    }
                });
            }
            Command::SetWriteMode(armed) => {
                write_armed = armed;
                reporter.send(Event::WriteMode(armed));
            }
            Command::Execute(action) => {
                execute(&mut client, &reporter, write_armed, *action).await;
            }
            Command::ExecuteBatch(actions) => {
                execute_batch(&mut client, &reporter, write_armed, actions).await;
            }
            Command::ExportToDatabase { password, tables } => {
                export_to_database(&config_for_export, &reporter, &password, tables).await;
            }
        }
    }
}

/// Perform one mutation, or refuse it.
///
/// This is the boundary. Because the access token carries write scopes for the
/// whole session, nothing else stops a mutation reaching Graph — so the check
/// lives here, at the single point every write must pass through, rather than
/// being repeated at each button in the UI where one could be forgotten.
async fn execute<F: Fn() + Send + 'static>(
    client: &mut GraphClient,
    reporter: &Reporter<F>,
    write_armed: bool,
    action: Action,
) {
    if !write_armed {
        reporter.send(Event::WriteRejected(action.label()));
        return;
    }

    let actor = client.account();
    // Written before the call: an action that never returns still leaves a trace.
    actionlog::record_attempt(&action, actor.as_deref());

    let result = action
        .execute(client)
        .await
        .map_err(|err| format!("{err:#}"));

    actionlog::record_outcome(&action, actor.as_deref(), &result);

    reporter.send(Event::ActionResult {
        label: action.label(),
        result: result.clone(),
    });

    // Re-read the affected collection so the console reflects reality rather
    // than what we assume the write did.
    if result.is_ok() {
        load_one(client, reporter, action.collection()).await;
    }
}

/// Run a batch of mutations one after another.
///
/// Sequential on purpose. Firing these in parallel would provoke Graph's
/// throttling and, worse, make a partial failure impossible to reason about —
/// the operator needs to know exactly which objects changed. A failure does not
/// abort the run: the remaining items still get their chance, and everything
/// that went wrong is reported together at the end.
async fn execute_batch<F: Fn() + Send + 'static>(
    client: &mut GraphClient,
    reporter: &Reporter<F>,
    write_armed: bool,
    actions: Vec<Action>,
) {
    if !write_armed {
        let label = actions
            .first()
            .map(Action::label)
            .unwrap_or_else(|| "batch".into());
        reporter.send(Event::WriteRejected(label));
        return;
    }

    let total = actions.len();
    let actor = client.account();
    let mut failures = Vec::new();
    let mut succeeded = 0;
    // Collections touched, so each is refreshed once at the end rather than
    // after every single item.
    let mut touched: Vec<Collection> = Vec::new();

    for (index, action) in actions.iter().enumerate() {
        actionlog::record_attempt(action, actor.as_deref());

        let result = action
            .execute(client)
            .await
            .map_err(|err| format!("{err:#}"));

        actionlog::record_outcome(action, actor.as_deref(), &result);

        match result {
            Ok(()) => {
                succeeded += 1;
                if !touched.contains(&action.collection()) {
                    touched.push(action.collection());
                }
            }
            Err(message) => failures.push((action.label(), message)),
        }

        reporter.send(Event::BatchProgress {
            done: index + 1,
            total,
        });
    }

    reporter.send(Event::BatchDone {
        succeeded,
        failures,
    });

    for collection in touched {
        load_one(client, reporter, collection).await;
    }
}

/// Replace the configured MariaDB tables.
///
/// Runs here rather than on the UI thread for the same reason every Graph call
/// does: a database on the other side of a VPN can take a long time to answer,
/// and a frozen window is indistinguishable from a crashed one.
async fn export_to_database<F: Fn() + Send + 'static>(
    settings: &Option<crate::config::MariaDb>,
    reporter: &Reporter<F>,
    password: &crate::mariadb::Secret,
    tables: Vec<crate::mariadb::Table>,
) {
    let Some(settings) = settings else {
        // Unreachable through the UI, which hides the command entirely when
        // the section is absent — but the worker does not assume that.
        reporter.send(Event::DatabaseDone(Err(
            "no [mariadb] section is configured".into(),
        )));
        return;
    };

    let total: usize = tables.iter().map(|table| table.rows.len()).sum();
    let count = tables.len();

    // Progress has to cross from the async export back to the UI, and the
    // reporter cannot be captured by the callback without borrowing it twice,
    // so the tables are announced through a channel instead.
    let (progress_tx, mut progress_rx) = tokio_mpsc::unbounded_channel();
    let result = {
        let exporting = crate::mariadb::export(settings, password, tables, move |progress| {
            let _ = progress_tx.send(progress);
        });
        tokio::pin!(exporting);
        loop {
            tokio::select! {
                outcome = &mut exporting => break outcome,
                Some(progress) = progress_rx.recv() => {
                    reporter.send(Event::DatabaseProgress {
                        table: progress.table,
                        rows: progress.rows,
                    });
                }
            }
        }
    };
    // Anything queued in the moment the export finished.
    while let Ok(progress) = progress_rx.try_recv() {
        reporter.send(Event::DatabaseProgress {
            table: progress.table,
            rows: progress.rows,
        });
    }

    reporter.send(Event::DatabaseDone(match result {
        Ok(written) => Ok(format!(
            "{total} rows into {count} tables ({}) at {}",
            written.join(", "),
            settings.describe()
        )),
        // `{err:#}` walks the context chain, which is what carries "connecting
        // to …" or "writing gcm_users" alongside the driver's own message.
        Err(err) => Err(format!("{err:#}")),
    }));
}

/// Returns true when sign-in succeeded.
async fn sign_in<F: Fn() + Send + 'static>(
    client: &mut GraphClient,
    reporter: &Reporter<F>,
) -> bool {
    let (progress_tx, mut progress_rx) = tokio_mpsc::unbounded_channel::<AuthProgress>();

    // Relay device-code prompts to the UI while sign_in is still awaiting.
    let relay_tx = reporter.tx.clone();
    let relay = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            let event = match progress {
                AuthProgress::Silent => Event::SigningInSilently,
                AuthProgress::AwaitingBrowser { url } => Event::AwaitingBrowser { url },
            };
            if relay_tx.send(event).is_err() {
                break;
            }
        }
    });

    // The relay task cannot call `repaint` (it is not Send-cloneable here), so
    // nudge egui on a short timer while sign-in is outstanding.
    let result = {
        let signing_in = client.auth_mut().sign_in(progress_tx);
        tokio::pin!(signing_in);
        loop {
            tokio::select! {
                outcome = &mut signing_in => break outcome,
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                    (reporter.repaint)();
                }
            }
        }
    };

    relay.abort();

    match result {
        Ok(()) => {
            reporter.send(Event::SignedIn {
                account: client.account(),
                writes_available: client.auth_mut().writes_available(),
            });
            true
        }
        Err(err) => {
            reporter.send(Event::Fatal(format!("Sign-in failed: {err:#}")));
            false
        }
    }
}

async fn load_all<F: Fn() + Send + 'static>(client: &mut GraphClient, reporter: &Reporter<F>) {
    // Tenant details first: the Intune result below is interpreted against it.
    match client.organization().await {
        Ok(org) => reporter.send(Event::Organization(Box::new(org))),
        Err(err) => reporter.send(Event::Failed {
            collection: None,
            message: format!("Could not read tenant details: {err:#}"),
        }),
    }

    // Directory first, then the workloads, then the logs. The order is what an
    // operator sees fill in, and the logs are both the slowest and the least
    // likely to be what somebody opened the console for.
    for collection in [
        Collection::Users,
        Collection::Groups,
        Collection::Roles,
        Collection::Devices,
        Collection::ManagedDevices,
        Collection::Licenses,
        Collection::Mailboxes,
        Collection::Teams,
        Collection::SignIns,
        Collection::AuditLogs,
    ] {
        load_one(client, reporter, collection).await;
    }
}

async fn load_one<F: Fn() + Send + 'static>(
    client: &mut GraphClient,
    reporter: &Reporter<F>,
    collection: Collection,
) {
    reporter.send(Event::Loading(collection));

    match collection {
        Collection::Users => {
            let result = client.users().await;
            reporter.report(collection, result, |users| Event::Users(Arc::new(users)));
        }
        Collection::Groups => {
            let result = client.groups().await;
            reporter.report(collection, result, |groups| Event::Groups(Arc::new(groups)));
        }
        Collection::Roles => {
            let result = client.directory_roles().await;
            reporter.report(collection, result, |roles| Event::Roles(Arc::new(roles)));
        }
        Collection::Devices => {
            let result = client.devices().await;
            reporter.report(collection, result, |devices| {
                Event::Devices(Arc::new(devices))
            });
        }
        Collection::ManagedDevices => {
            let result = client.managed_devices().await;
            reporter.report(collection, result, |fetch| {
                Event::ManagedDevices(share(fetch))
            });
        }
        Collection::Licenses => {
            let result = client.subscribed_skus().await;
            reporter.report(collection, result, |skus| Event::Licenses(Arc::new(skus)));
        }
        Collection::SignIns => {
            let result = client.sign_ins().await;
            reporter.report(collection, result, |fetch| {
                Event::SignIns(share(fetch))
            });
        }
        Collection::AuditLogs => {
            let result = client.directory_audits().await;
            reporter.report(collection, result, |fetch| {
                Event::AuditLogs(share(fetch))
            });
        }
        Collection::Teams => {
            let result = client.teams().await;
            reporter.report(collection, result, |fetch| Event::Teams(share(fetch)));
        }
        Collection::Mailboxes => {
            let result = client.mailboxes().await;
            reporter.report(collection, result, |fetch| {
                Event::Mailboxes(share(fetch))
            });
        }
    }
}

/// Move a fetched collection behind an `Arc` so the UI can clone a handle per
/// frame rather than the rows themselves.
fn share<T>(fetch: Fetch<Vec<T>>) -> Fetch<Arc<Vec<T>>> {
    match fetch {
        Fetch::Ready(items) => Fetch::Ready(Arc::new(items)),
        Fetch::Unavailable(reason) => Fetch::Unavailable(reason),
    }
}

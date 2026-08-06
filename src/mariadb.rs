//! Writing the console's collections into MariaDB.
//!
//! The file exports ([`crate::ui::export`]) write exactly what is on screen —
//! same columns, same filter, same order — because somebody asked for *this
//! list*. A database export is a different errand: it feeds a reporting schema
//! that other queries join against, and a table silently narrowed by whatever
//! was typed into the filter box twenty minutes ago would be a trap. So this
//! writes **every row of every loaded collection**, unfiltered, and says so in
//! the confirmation.
//!
//! ## How a table is replaced
//!
//! MariaDB commits implicitly on DDL, so "wrap the whole thing in a
//! transaction" is not available: a `CREATE TABLE` mid-transaction would commit
//! everything before it. Instead each table is built beside the real one and
//! swapped in:
//!
//! 1. `CREATE OR REPLACE TABLE gcm_users__staging (…)`
//! 2. batched `INSERT`s, inside a transaction
//! 3. `RENAME TABLE gcm_users TO gcm_users__old, gcm_users__staging TO gcm_users`
//! 4. `DROP TABLE gcm_users__old`
//!
//! `RENAME TABLE` is atomic, so a dashboard reading `gcm_users` at any moment
//! sees either the whole previous export or the whole new one — never a table
//! mid-refill, and never a missing one. Step 3 degrades to a plain rename the
//! first time, when there is nothing to swap out.
//!
//! ## Types
//!
//! Column types are inferred from the values actually being written rather than
//! declared per view, so `assigned` arrives as `BIGINT` and can be summed, and
//! `when` arrives as `DATETIME` and can be ordered. Inference is deliberately
//! conservative: a single value that does not fit makes the whole column
//! `TEXT`, because a column that is nearly a number is worse than one that is
//! honestly a string.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use mysql_async::prelude::*;

use crate::config::MariaDb;

/// How long to wait for the initial connection before giving up.
///
/// `mysql_async` sets no timeout of its own, so without this an unreachable
/// host — a wrong bridged-adapter IP, a firewall that drops packets instead
/// of refusing them — hangs the export forever: no error, nothing written,
/// and nothing on screen to say either has happened.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A password, which cannot be printed by accident.
///
/// It travels inside a [`crate::worker::Command`], and `Command` derives
/// `Debug` so the whole enum can be logged or inspected. A bare `String` there
/// would be one `{:?}` away from putting a database password in the error log,
/// which is precisely the file people email to other people. The manual `Debug`
/// makes that impossible rather than merely unlikely.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Rows to write for one collection, already rendered to display strings by the
/// same accessors the table and the CSV export use.
#[derive(Debug, Clone)]
pub struct Table {
    /// Table name without the configured prefix — `users`, `sign_ins`.
    pub stem: &'static str,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// What a column will be declared as.
///
/// Ordered from most specific to least: inference starts optimistic and widens
/// as it meets values that do not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ColumnType {
    Integer,
    Double,
    DateTime,
    Date,
    Text,
}

impl ColumnType {
    fn sql(self) -> &'static str {
        match self {
            ColumnType::Integer => "BIGINT",
            ColumnType::Double => "DOUBLE",
            ColumnType::DateTime => "DATETIME",
            ColumnType::Date => "DATE",
            // utf8mb4 throughout: display names contain emoji more often than
            // anyone expects, and latin1 would mangle them.
            ColumnType::Text => "TEXT",
        }
    }

    /// Whether a single value fits this type.
    fn accepts(self, value: &str) -> bool {
        match self {
            ColumnType::Integer => value.parse::<i64>().is_ok(),
            ColumnType::Double => value.parse::<f64>().is_ok(),
            ColumnType::DateTime => {
                chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").is_ok()
                    || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").is_ok()
            }
            ColumnType::Date => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok(),
            ColumnType::Text => true,
        }
    }
}

/// A value the console renders as absent. Written as SQL `NULL`, so that
/// `WHERE last_activity IS NULL` means what it looks like and an em dash never
/// reaches the database.
fn is_null(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value == "—" || value == "n/a"
}

/// Work out what a column should be declared as, from every value in it.
fn infer_from<'a>(values: impl Iterator<Item = &'a str>) -> ColumnType {
    let candidates = [
        ColumnType::Integer,
        ColumnType::Double,
        ColumnType::DateTime,
        ColumnType::Date,
    ];
    let mut alive = [true; 4];
    let mut saw_a_value = false;

    for value in values {
        if is_null(value) {
            continue;
        }
        saw_a_value = true;
        for (index, candidate) in candidates.iter().enumerate() {
            if alive[index] && !candidate.accepts(value.trim()) {
                alive[index] = false;
            }
        }
        if !alive.iter().any(|ok| *ok) {
            return ColumnType::Text;
        }
    }

    // A column that is entirely empty tells us nothing, and guessing BIGINT
    // would make the next export fail the moment a real value turned up.
    if !saw_a_value {
        return ColumnType::Text;
    }

    candidates
        .iter()
        .zip(alive)
        .find(|(_, ok)| *ok)
        .map(|(kind, _)| *kind)
        .unwrap_or(ColumnType::Text)
}

/// Turn a display heading into a column name.
///
/// Lower-cased, non-alphanumerics collapsed to single underscores. A name that
/// would start with a digit is prefixed, because MariaDB will not accept it.
pub fn column_name(heading: &str) -> String {
    let mut out = String::with_capacity(heading.len());
    for c in heading.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    match trimmed.chars().next() {
        None => "column".into(),
        Some(first) if first.is_ascii_digit() => format!("c_{trimmed}"),
        _ => trimmed,
    }
}

/// Quote an identifier for MariaDB.
///
/// Every identifier here is derived from a `&'static str` heading or from the
/// validated table prefix, so none is attacker-controlled — but backticking is
/// what makes a heading like `Type` (a reserved word) work at all, and the
/// doubling is the standard escape.
fn quote(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

/// Names that must not collide with the tables gcm manages.
fn staging_name(table: &str) -> String {
    format!("{table}__staging")
}

fn retiring_name(table: &str) -> String {
    format!("{table}__old")
}

/// How many rows to put in one multi-row `INSERT`.
///
/// Large enough that a fifty-thousand-user tenant is not fifty thousand round
/// trips, small enough to stay well inside the default `max_allowed_packet` of
/// 16 MB even with wide rows.
const BATCH: usize = 500;

/// Progress, reported per table so the UI can say what it is doing.
pub struct Progress {
    pub table: String,
    pub rows: usize,
}

/// Write every table, replacing what was there.
///
/// `report` is called as each table lands. Returns the tables written, in order.
pub async fn export(
    settings: &MariaDb,
    password: &Secret,
    tables: Vec<Table>,
    mut report: impl FnMut(Progress),
) -> Result<Vec<String>> {
    if tables.is_empty() {
        bail!("there is nothing loaded to export yet");
    }

    let mut options = mysql_async::OptsBuilder::from_opts(
        mysql_async::Opts::from_url(&settings.url(password.expose()))
            // The URL carries the password, so its parse error must not be
            // propagated verbatim — it would quote the whole URL back.
            .map_err(|_| {
                anyhow::anyhow!(
                    "could not build a connection for {}. Check host, port, user and \
                     database in the [mariadb] section of the configuration.",
                    settings.describe()
                )
            })?,
    );

    if settings.require_tls {
        // Verification left on. An export carries the entire directory across
        // this connection; accepting any certificate would make the encryption
        // decorative.
        options = options.ssl_opts(mysql_async::SslOpts::default());
    }

    let pool = mysql_async::Pool::new(options);
    let mut conn = tokio::time::timeout(CONNECT_TIMEOUT, pool.get_conn())
        .await
        .with_context(|| {
            format!(
                "connecting to {} timed out after {}s — check the host, port and that \
                 nothing is silently dropping the connection",
                settings.describe(),
                CONNECT_TIMEOUT.as_secs()
            )
        })?
        .with_context(|| format!("connecting to {}", settings.describe()))?;

    let mut written = Vec::new();
    for table in tables {
        let name = settings.table_for(table.stem);
        let rows = table.rows.len();
        write_table(&mut conn, &name, &table)
            .await
            .with_context(|| format!("writing {name}"))?;
        report(Progress {
            table: name.clone(),
            rows,
        });
        written.push(name);
    }

    drop(conn);
    // Closing the pool rather than letting it drop: an abandoned pool logs a
    // warning about being dropped in an async context on some runtimes.
    let _ = pool.disconnect().await;

    Ok(written)
}

/// Build one table beside the live one and swap it in.
async fn write_table(
    conn: &mut mysql_async::Conn,
    name: &str,
    table: &Table,
) -> Result<()> {
    let staging = staging_name(name);
    let retiring = retiring_name(name);

    // Types come from the data, so an empty collection still produces a table —
    // with TEXT columns, which is the honest answer when there is nothing to
    // infer from.
    let types: Vec<ColumnType> = (0..table.columns.len())
        .map(|index| {
            infer_from(
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(index))
                    .map(String::as_str),
            )
        })
        .collect();

    let definitions: Vec<String> = table
        .columns
        .iter()
        .zip(&types)
        .map(|(heading, kind)| format!("{} {}", quote(&column_name(heading)), kind.sql()))
        .collect();

    conn.query_drop(format!(
        "CREATE OR REPLACE TABLE {} (\n  {}\n) ENGINE=InnoDB \
         DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        quote(&staging),
        definitions.join(",\n  ")
    ))
    .await
    .context("creating the staging table")?;

    if !table.rows.is_empty() {
        let columns: Vec<String> = table
            .columns
            .iter()
            .map(|heading| quote(&column_name(heading)))
            .collect();

        let mut transaction = conn
            .start_transaction(mysql_async::TxOpts::default())
            .await
            .context("starting the insert transaction")?;

        for chunk in table.rows.chunks(BATCH) {
            // One placeholder group per row. Values are always parameters, never
            // interpolated — the strings here come from a tenant's directory and
            // include quotes, backslashes and semicolons as a matter of course.
            let group = format!("({})", vec!["?"; columns.len()].join(", "));
            let statement = format!(
                "INSERT INTO {} ({}) VALUES {}",
                quote(&staging),
                columns.join(", "),
                vec![group; chunk.len()].join(", ")
            );

            let mut params: Vec<mysql_async::Value> =
                Vec::with_capacity(chunk.len() * columns.len());
            for row in chunk {
                // Walked by column rather than over the row, so a short row —
                // which should not happen, but would silently shift every later
                // value if it did — pads with NULL instead of misaligning.
                for (index, kind) in types.iter().enumerate() {
                    let raw = row.get(index).map(String::as_str).unwrap_or("");
                    params.push(bind(raw, *kind));
                }
            }

            transaction
                .exec_drop(statement, params)
                .await
                .context("inserting rows")?;
        }

        transaction.commit().await.context("committing the rows")?;
    }

    // Atomic swap. RENAME TABLE moves both in one statement, so nothing reading
    // the live table ever sees it absent or half-filled.
    let exists: Option<i64> = conn
        .exec_first(
            "SELECT 1 FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
            (name,),
        )
        .await
        .context("checking whether the table already exists")?;

    if exists.is_some() {
        conn.query_drop(format!(
            "RENAME TABLE {} TO {}, {} TO {}",
            quote(name),
            quote(&retiring),
            quote(&staging),
            quote(name)
        ))
        .await
        .context("swapping the new table in")?;
        conn.query_drop(format!("DROP TABLE IF EXISTS {}", quote(&retiring)))
            .await
            .context("dropping the previous table")?;
    } else {
        conn.query_drop(format!(
            "RENAME TABLE {} TO {}",
            quote(&staging),
            quote(name)
        ))
        .await
        .context("naming the new table")?;
    }

    Ok(())
}

/// Bind one display string as the column's declared type.
///
/// A value that does not parse falls back to NULL rather than to zero: a
/// mailbox whose item count could not be read is unknown, not empty.
fn bind(raw: &str, kind: ColumnType) -> mysql_async::Value {
    use mysql_async::Value;

    if is_null(raw) {
        return Value::NULL;
    }
    let value = raw.trim();

    match kind {
        ColumnType::Integer => value
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or(Value::NULL),
        ColumnType::Double => value
            .parse::<f64>()
            .map(Value::Double)
            .unwrap_or(Value::NULL),
        ColumnType::DateTime => chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S"))
            .map(Value::from)
            .unwrap_or(Value::NULL),
        ColumnType::Date => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map(Value::from)
            .unwrap_or(Value::NULL),
        ColumnType::Text => Value::Bytes(value.as_bytes().to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infer_values(values: &[&str]) -> ColumnType {
        infer_from(values.iter().copied())
    }

    #[test]
    fn headings_become_usable_column_names() {
        assert_eq!(column_name("Name"), "name");
        assert_eq!(column_name("User principal name"), "user_principal_name");
        assert_eq!(column_name("SKU part number"), "sku_part_number");
        assert_eq!(column_name("Last check-in"), "last_check_in");
        assert_eq!(column_name("Prohibit Send/Receive Quota"), "prohibit_send_receive_quota");
    }

    #[test]
    fn column_names_never_start_with_a_digit() {
        // MariaDB rejects a bare identifier that does, and every heading in the
        // console is ours to change tomorrow.
        assert_eq!(column_name("2FA enabled"), "c_2fa_enabled");
        assert_eq!(column_name("!!!"), "column");
        assert_eq!(column_name(""), "column");
    }

    #[test]
    fn identifiers_are_backticked_and_escaped() {
        assert_eq!(quote("users"), "`users`");
        // A backtick in an identifier is doubled, per MariaDB's own escaping.
        assert_eq!(quote("we`ird"), "`we``ird`");
    }

    #[test]
    fn numeric_columns_are_inferred() {
        // So that SUM(assigned) works rather than summing strings.
        assert_eq!(infer_values(&["231", "40", "0"]), ColumnType::Integer);
        assert_eq!(infer_values(&["1.5", "2", "0.25"]), ColumnType::Double);
    }

    #[test]
    fn date_columns_are_inferred() {
        assert_eq!(
            infer_values(&["2026-08-05 19:01", "2026-08-04 07:22"]),
            ColumnType::DateTime
        );
        assert_eq!(infer_values(&["2026-08-05", "2026-08-04"]), ColumnType::Date);
    }

    #[test]
    fn one_bad_value_widens_the_whole_column_to_text() {
        // Nearly-a-number is worse than honestly-a-string: it makes the schema
        // depend on which tenant was exported first.
        assert_eq!(infer_values(&["231", "40", "unlimited"]), ColumnType::Text);
        assert_eq!(
            infer_values(&["2026-08-05 19:01", "never"]),
            ColumnType::Text
        );
    }

    #[test]
    fn absent_values_do_not_influence_the_type() {
        // The console renders absence as an em dash; that must not make an
        // otherwise numeric column into text.
        assert_eq!(infer_values(&["231", "—", "40"]), ColumnType::Integer);
        assert_eq!(infer_values(&["2026-08-05", "", "2026-08-04"]), ColumnType::Date);
    }

    #[test]
    fn a_column_with_nothing_in_it_stays_text() {
        // Guessing a type from no evidence would make the next export fail as
        // soon as a real value appeared.
        assert_eq!(infer_values(&["—", "", "n/a"]), ColumnType::Text);
        assert_eq!(infer_values(&[]), ColumnType::Text);
    }

    #[test]
    fn absent_values_bind_as_null_not_as_zero() {
        // A mailbox whose item count could not be read is unknown, not empty,
        // and a dashboard averaging the column must not be told otherwise.
        assert_eq!(bind("—", ColumnType::Integer), mysql_async::Value::NULL);
        assert_eq!(bind("", ColumnType::Text), mysql_async::Value::NULL);
        assert_eq!(bind("n/a", ColumnType::Date), mysql_async::Value::NULL);
    }

    #[test]
    fn values_bind_as_their_declared_type() {
        assert_eq!(bind("231", ColumnType::Integer), mysql_async::Value::Int(231));
        assert_eq!(
            bind("Aisha Rahman", ColumnType::Text),
            mysql_async::Value::Bytes(b"Aisha Rahman".to_vec())
        );
    }

    #[test]
    fn a_value_that_will_not_parse_becomes_null_rather_than_wrong() {
        // Reachable when a column is typed from one export's data and a later
        // one carries something unexpected in it.
        assert_eq!(bind("lots", ColumnType::Integer), mysql_async::Value::NULL);
    }

    #[test]
    fn staging_and_retiring_names_are_distinct_from_the_real_one() {
        // They share the prefix, so they are recognisably gcm's, but nothing
        // that collides with a table an export actually publishes.
        let name = "gcm_users";
        assert_ne!(staging_name(name), name);
        assert_ne!(retiring_name(name), name);
        assert_ne!(staging_name(name), retiring_name(name));
        assert!(staging_name(name).starts_with(name));
    }

    #[test]
    fn text_is_the_widest_type() {
        // Inference walks from specific to general and must be able to stop.
        assert!(ColumnType::Text > ColumnType::Integer);
        assert!(ColumnType::Text.accepts("anything at all"));
    }
}

//! Writing the current view out as CSV or JSON.
//!
//! Export reads through the same [`columns`](super::list::columns) and cell
//! accessors the table renders from, so what lands in the file is exactly what
//! was on screen — same columns, same filter, same order. An export that
//! quietly differs from the view it came from is worse than no export, because
//! nobody checks.

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::{App, View, list};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Json => "json",
        }
    }
}

/// A filename that says what the export is and when it was taken.
fn suggested_name(view: View, format: Format) -> String {
    let stem = view
        .title()
        .to_lowercase()
        .replace(' ', "-")
        .replace(['(', ')'], "");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M");
    format!("gcm-{stem}-{stamp}.{}", format.extension())
}

/// Render the rows currently visible in `view`.
///
/// Only the filtered set is written: the file should match what the operator
/// was looking at when they asked for it.
fn rows(app: &App, view: View) -> (Vec<&'static str>, Vec<Vec<String>>) {
    let columns = list::columns(view);
    let headers: Vec<&'static str> = columns.iter().map(|column| column.title).collect();

    let sources: Vec<usize> = app
        .views
        .get(&view)
        .map(|state| state.filtered.clone())
        .unwrap_or_default();

    let rows = sources
        .into_iter()
        .map(|source| list::export_row(app, view, source))
        .collect();

    (headers, rows)
}

/// Every loaded collection, rendered for the database export.
///
/// Two differences from [`rows`], both deliberate:
///
/// * **Every view, not just the current one.** A reporting schema wants the
///   whole tenant in one refresh, not whichever node happened to be selected.
/// * **Every row, not the filtered set.** A file export answers "give me *this
///   list*", so the filter belongs in it. A database table narrowed by whatever
///   somebody typed twenty minutes ago would be a trap for every query that
///   later joins against it.
///
/// Collections the tenant does not offer are skipped rather than written empty:
/// an empty `gcm_teams` would read as "this tenant has no teams", which is a
/// different and wrong statement.
pub fn database_tables(app: &App) -> Vec<crate::mariadb::Table> {
    View::ALL
        .iter()
        .filter_map(|view| {
            let stem = view.table_stem()?;
            // Never loaded, or refused by the tenant.
            if app.store.unavailable(*view).is_some() {
                return None;
            }
            let count = app.store.count(*view)?;

            let columns = list::columns(*view)
                .iter()
                .map(|column| column.title.to_string())
                .collect();
            let rows = (0..count)
                .map(|source| list::export_row(app, *view, source))
                .collect();

            Some(crate::mariadb::Table {
                stem,
                columns,
                rows,
            })
        })
        .collect()
}

fn to_csv(headers: &[&str], rows: &[Vec<String>]) -> Result<String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(headers).context("writing the header")?;
    for row in rows {
        writer.write_record(row).context("writing a row")?;
    }
    let bytes = writer.into_inner().context("finishing the CSV")?;
    String::from_utf8(bytes).context("the CSV was not valid UTF-8")
}

fn to_json(headers: &[&str], rows: &[Vec<String>]) -> Result<String> {
    let records: Vec<serde_json::Map<String, serde_json::Value>> = rows
        .iter()
        .map(|row| {
            headers
                .iter()
                .zip(row)
                .map(|(header, value)| {
                    (header.to_string(), serde_json::Value::String(value.clone()))
                })
                .collect()
        })
        .collect();
    serde_json::to_string_pretty(&records).context("rendering JSON")
}

/// Ask where to save, then write. Returns the path written.
///
/// Returns `Ok(None)` when the operator cancelled the dialog, which is not an
/// error and should not be reported as one.
pub fn save(app: &App, view: View, format: Format) -> Result<Option<PathBuf>> {
    let (headers, rows) = rows(app, view);
    let body = match format {
        Format::Csv => to_csv(&headers, &rows)?,
        Format::Json => to_json(&headers, &rows)?,
    };

    let chosen = rfd::FileDialog::new()
        .set_file_name(suggested_name(view, format))
        .add_filter(
            match format {
                Format::Csv => "Comma-separated values",
                Format::Json => "JSON",
            },
            &[format.extension()],
        )
        .save_file();

    let Some(path) = chosen else {
        return Ok(None);
    };

    std::fs::write(&path, body)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_values_that_would_break_the_format() {
        let headers = ["Name", "Note"];
        let rows = vec![
            vec!["Plain".to_string(), "no punctuation".to_string()],
            vec!["Comma".to_string(), "Finance, Legal".to_string()],
            vec!["Quote".to_string(), "she said \"hello\"".to_string()],
            vec!["Newline".to_string(), "line one\nline two".to_string()],
        ];

        let csv = to_csv(&headers, &rows).expect("should render");

        // A bare comma would silently create an extra column.
        assert!(csv.contains("\"Finance, Legal\""));
        // Embedded quotes are doubled, per RFC 4180.
        assert!(csv.contains("\"she said \"\"hello\"\"\""));
        assert!(csv.contains("\"line one\nline two\""));
        // Values needing no quoting are left alone.
        assert!(csv.contains("Plain,no punctuation"));
    }

    #[test]
    fn csv_round_trips_through_a_reader() {
        let headers = ["Name", "Department"];
        let rows = vec![vec![
            "Aisha Rahman".to_string(),
            "Finance, Group".to_string(),
        ]];
        let csv = to_csv(&headers, &rows).expect("should render");

        let mut reader = csv::Reader::from_reader(csv.as_bytes());
        let parsed: Vec<Vec<String>> = reader
            .records()
            .map(|record| {
                record
                    .expect("row should parse")
                    .iter()
                    .map(String::from)
                    .collect()
            })
            .collect();

        assert_eq!(parsed, rows);
    }

    #[test]
    fn json_pairs_every_value_with_its_header() {
        let headers = ["Name", "Status"];
        let rows = vec![vec!["Ben Okafor".to_string(), "Enabled".to_string()]];
        let json = to_json(&headers, &rows).expect("should render");

        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed[0]["Name"], "Ben Okafor");
        assert_eq!(parsed[0]["Status"], "Enabled");
    }

    #[test]
    fn suggested_names_are_descriptive_and_safe() {
        let name = suggested_name(View::ManagedDevices, Format::Csv);
        assert!(name.starts_with("gcm-managed-devices-intune-"));
        assert!(name.ends_with(".csv"));
        // Parentheses from the view title would be awkward in a filename.
        assert!(!name.contains('(') && !name.contains(')'));
    }
}

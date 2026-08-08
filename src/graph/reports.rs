//! Parsing the Microsoft 365 usage reports, which arrive as CSV rather than as
//! an OData collection.
//!
//! Graph has no collection that lists every mailbox — `/users` knows about
//! accounts, not about the mailboxes behind them. `getMailboxUsageDetail` is
//! the only v1.0 endpoint that does, and it answers with a `302` to a
//! short-lived download URL serving a CSV file.
//!
//! Two things about that file drive the shape of this module:
//!
//! * **Columns are addressed by name, never by position.** Microsoft's own
//!   documentation shows two different column lists for the same report, and
//!   the set has grown over time. Indexing by position would silently read the
//!   wrong field the next time a column is inserted.
//! * **Every column is optional.** A tenant that has never provisioned archive
//!   mailboxes omits `Has Archive` entirely, so a missing column is a normal
//!   state rather than a parse failure.

use anyhow::{Context, Result};
use chrono::NaiveDate;

use super::models::{Mailbox, MailboxSource};

/// Column headings this parser knows how to read, in the order the details
/// pane presents them. Anything else in the file is ignored.
const UPN: &str = "User Principal Name";
const DISPLAY_NAME: &str = "Display Name";
const IS_DELETED: &str = "Is Deleted";
const CREATED: &str = "Created Date";
const LAST_ACTIVITY: &str = "Last Activity Date";
const ITEM_COUNT: &str = "Item Count";
const STORAGE_USED: &str = "Storage Used (Byte)";
const WARNING_QUOTA: &str = "Issue Warning Quota (Byte)";
const SEND_QUOTA: &str = "Prohibit Send Quota (Byte)";
const SEND_RECEIVE_QUOTA: &str = "Prohibit Send/Receive Quota (Byte)";
const DELETED_ITEM_COUNT: &str = "Deleted Item Count";
const DELETED_ITEM_SIZE: &str = "Deleted Item Size (Byte)";
const HAS_ARCHIVE: &str = "Has Archive";

/// Read a `getMailboxUsageDetail` CSV into mailboxes.
///
/// Rows are returned in the order the report supplied them; the caller sorts.
pub fn parse_mailbox_usage(csv_text: &str) -> Result<Vec<Mailbox>> {
    // The report is served UTF-8 with a byte-order mark, which would otherwise
    // become part of the first column's name and lose the UPN column.
    let csv_text = csv_text.trim_start_matches('\u{feff}');

    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers = reader
        .headers()
        .context("reading the report's column headings")?
        .clone();

    // Map heading -> column index once, rather than per row.
    let column = |name: &str| headers.iter().position(|heading| heading.trim() == name);
    let columns = Columns {
        upn: column(UPN),
        display_name: column(DISPLAY_NAME),
        is_deleted: column(IS_DELETED),
        created: column(CREATED),
        last_activity: column(LAST_ACTIVITY),
        item_count: column(ITEM_COUNT),
        storage_used: column(STORAGE_USED),
        warning_quota: column(WARNING_QUOTA),
        send_quota: column(SEND_QUOTA),
        send_receive_quota: column(SEND_RECEIVE_QUOTA),
        deleted_item_count: column(DELETED_ITEM_COUNT),
        deleted_item_size: column(DELETED_ITEM_SIZE),
        has_archive: column(HAS_ARCHIVE),
    };

    let mut mailboxes = Vec::new();
    for record in reader.records() {
        let record = record.context("reading a row of the report")?;
        let field = |index: Option<usize>| -> &str {
            index.and_then(|i| record.get(i)).unwrap_or("").trim()
        };

        let mailbox = Mailbox {
            user_principal_name: field(columns.upn).to_string(),
            display_name: field(columns.display_name).to_string(),
            is_deleted: parse_bool(field(columns.is_deleted)).unwrap_or(false),
            created: parse_date(field(columns.created)),
            last_activity: parse_date(field(columns.last_activity)),
            item_count: parse_i64(field(columns.item_count)),
            storage_used: parse_i64(field(columns.storage_used)),
            issue_warning_quota: parse_i64(field(columns.warning_quota)),
            prohibit_send_quota: parse_i64(field(columns.send_quota)),
            prohibit_send_receive_quota: parse_i64(field(columns.send_receive_quota)),
            deleted_item_count: parse_i64(field(columns.deleted_item_count)),
            deleted_item_size: parse_i64(field(columns.deleted_item_size)),
            has_archive: parse_bool(field(columns.has_archive)),
            source: MailboxSource::Report,
        };

        // A row with no identity at all is a trailing blank line, not a mailbox.
        if mailbox.user_principal_name.is_empty() && mailbox.display_name.is_empty() {
            continue;
        }
        mailboxes.push(mailbox);
    }

    Ok(mailboxes)
}

/// Where each known heading sits in this particular file.
struct Columns {
    upn: Option<usize>,
    display_name: Option<usize>,
    is_deleted: Option<usize>,
    created: Option<usize>,
    last_activity: Option<usize>,
    item_count: Option<usize>,
    storage_used: Option<usize>,
    warning_quota: Option<usize>,
    send_quota: Option<usize>,
    send_receive_quota: Option<usize>,
    deleted_item_count: Option<usize>,
    deleted_item_size: Option<usize>,
    has_archive: Option<usize>,
}

/// A blank cell means "no value", which for a count is zero rather than an
/// error — a mailbox nobody has touched reports an empty item count.
fn parse_i64(value: &str) -> i64 {
    value.parse().unwrap_or(0)
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

/// The report writes booleans as `True`/`False`, but has also been observed
/// emitting `1`/`0`. Anything unrecognised stays unknown rather than
/// defaulting to false and asserting something untrue about a mailbox.
fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Report Refresh Date,User Principal Name,Display Name,Is Deleted,\
Deleted Date,Created Date,Last Activity Date,Item Count,Storage Used (Byte),\
Issue Warning Quota (Byte),Prohibit Send Quota (Byte),\
Prohibit Send/Receive Quota (Byte),Deleted Item Count,Deleted Item Size (Byte),\
Deleted Item Quota (Byte),Has Archive,Report Period
2026-08-04,aisha.rahman@contoso.co.uk,Aisha Rahman,False,,2022-01-04,2026-08-03,\
14203,21474836480,105226698752,107374182400,108447924224,412,1073741824,\
32212254720,True,7
2026-08-04,ben.okafor@contoso.co.uk,Ben Okafor,False,,2023-06-01,,0,0,\
105226698752,107374182400,108447924224,0,0,32212254720,False,7
";

    #[test]
    fn reads_the_documented_report_shape() {
        let mailboxes = parse_mailbox_usage(SAMPLE).expect("should parse");
        assert_eq!(mailboxes.len(), 2);

        let aisha = &mailboxes[0];
        assert_eq!(aisha.upn(), "aisha.rahman@contoso.co.uk");
        assert_eq!(aisha.name(), "Aisha Rahman");
        assert_eq!(aisha.item_count, 14203);
        assert_eq!(aisha.storage_used, 21474836480);
        assert_eq!(aisha.quota(), 108447924224);
        assert_eq!(aisha.has_archive, Some(true));
        assert_eq!(
            aisha.last_activity,
            NaiveDate::from_ymd_opt(2026, 8, 3)
        );
    }

    #[test]
    fn a_mailbox_never_used_has_no_last_activity() {
        let mailboxes = parse_mailbox_usage(SAMPLE).expect("should parse");
        assert_eq!(mailboxes[1].last_activity, None);
        assert_eq!(mailboxes[1].item_count, 0);
    }

    #[test]
    fn columns_are_found_by_name_not_position() {
        // The same report with its columns reordered and an unknown one added
        // must still read correctly — which is the whole point of the mapping.
        let shuffled = "Report Period,Has Archive,Display Name,Some Future Column,\
User Principal Name,Storage Used (Byte)\n\
7,True,Chloe Duval,ignored,chloe.duval@contoso.co.uk,1048576\n";
        let mailboxes = parse_mailbox_usage(shuffled).expect("should parse");
        assert_eq!(mailboxes.len(), 1);
        assert_eq!(mailboxes[0].upn(), "chloe.duval@contoso.co.uk");
        assert_eq!(mailboxes[0].storage_used, 1048576);
        assert_eq!(mailboxes[0].has_archive, Some(true));
    }

    #[test]
    fn missing_columns_are_not_a_failure() {
        // A tenant with no archive mailboxes omits the column entirely.
        let minimal = "User Principal Name,Display Name\n\
dmitri.sokolov@contoso.co.uk,Dmitri Sokolov\n";
        let mailboxes = parse_mailbox_usage(minimal).expect("should parse");
        assert_eq!(mailboxes.len(), 1);
        assert_eq!(mailboxes[0].has_archive, None);
        assert_eq!(mailboxes[0].storage_used, 0);
    }

    #[test]
    fn a_byte_order_mark_does_not_swallow_the_first_column() {
        let with_bom = "\u{feff}User Principal Name,Display Name\n\
grace.lin@contoso.co.uk,Grace Lin\n";
        let mailboxes = parse_mailbox_usage(with_bom).expect("should parse");
        assert_eq!(mailboxes[0].upn(), "grace.lin@contoso.co.uk");
    }

    #[test]
    fn blank_trailing_rows_are_dropped() {
        let trailing = "User Principal Name,Display Name\n\
liam.byrne@contoso.co.uk,Liam Byrne\n,\n";
        let mailboxes = parse_mailbox_usage(trailing).expect("should parse");
        assert_eq!(mailboxes.len(), 1);
    }

    #[test]
    fn unrecognised_booleans_stay_unknown() {
        // Better an em dash than a confident "No" about an archive that exists.
        assert_eq!(parse_bool(""), None);
        assert_eq!(parse_bool("maybe"), None);
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
    }
}

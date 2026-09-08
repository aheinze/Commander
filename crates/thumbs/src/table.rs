//! Bounded, read-only spreadsheet data prepared on the preview worker.

use std::io::Read;
use std::path::Path;

use super::{CancelToken, PreviewError, VPath, Vfs};

mod excel;
#[cfg(test)]
mod tests;

const MAX_ROWS: usize = 500;
const MAX_COLUMNS: usize = 50;
const MAX_SHEETS: usize = 20;
const MAX_CELL_BYTES: usize = 2048;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORKBOOK_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct TableDocument {
    pub format: &'static str,
    pub sheets: Vec<TableSheet>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TableSheet {
    pub name: String,
    /// Zero-based worksheet coordinates of the first displayed cell.
    pub first_row: u32,
    pub first_column: u32,
    pub columns: usize,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
    pub error: Option<String>,
}

pub(super) fn supports(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "csv" | "tsv" | "xlsx" | "xlsm" | "xlsb" | "xls"
    )
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub(super) fn load(
    vfs: &dyn Vfs,
    path: &VPath,
    cancel: &CancelToken,
) -> Result<TableDocument, PreviewError> {
    let extension = extension(path.as_path());
    let delimited = matches!(extension.as_str(), "csv" | "tsv");
    let limit = if delimited {
        MAX_TEXT_BYTES
    } else {
        MAX_WORKBOOK_BYTES
    };
    let mut bytes = Vec::new();
    vfs.open_read(path)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    check(cancel)?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    if delimited {
        load_csv(&bytes, extension == "tsv", truncated, cancel)
    } else if truncated {
        Err(error("file exceeds the 32 MiB workbook preview limit"))
    } else {
        excel::load(&bytes, &extension, cancel)
    }
}

fn check(cancel: &CancelToken) -> Result<(), PreviewError> {
    cancel.check().map_err(|_| PreviewError::Cancelled)
}

fn error(message: impl std::fmt::Display) -> PreviewError {
    PreviewError::Table(message.to_string())
}

/// Clips UTF-8 safely and counts the whole document's retained cell text.
fn cell_text(value: &str, budget: &mut usize, truncated: &mut bool) -> String {
    let limit = MAX_CELL_BYTES.min(*budget);
    let end = value.floor_char_boundary(limit.min(value.len()));
    let mut text = value[..end].replace('\0', "�");
    if end < value.len() {
        text.push('…');
        *truncated = true;
    }
    *budget = budget.saturating_sub(text.len());
    text
}

fn csv_text(bytes: &[u8], truncated: bool) -> Result<String, PreviewError> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let little = bytes[0] == 0xff;
        let words = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect::<Vec<_>>();
        if truncated {
            return Ok(String::from_utf16_lossy(&words));
        }
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err(error("incomplete UTF-16 text"));
        }
        return String::from_utf16(&words).map_err(|_| error("invalid UTF-16 text"));
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(failure) if truncated && failure.error_len().is_none() => {
            Ok(std::str::from_utf8(&bytes[..failure.valid_up_to()])
                .unwrap()
                .to_owned())
        }
        Err(_) => Err(error(
            "text encoding is not supported; save the file as UTF-8 or UTF-16",
        )),
    }
}

fn reader(text: &str, delimiter: u8) -> csv::Reader<&[u8]> {
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes())
}

fn delimiter(text: &str, tsv: bool) -> u8 {
    if tsv {
        return b'\t';
    }
    let sample = &text[..text.floor_char_boundary(text.len().min(64 * 1024))];
    let mut best = (0, 0);
    let mut selected = b',';
    for candidate in *b",;\t|" {
        let mut widths = std::collections::BTreeMap::<usize, usize>::new();
        for record in reader(sample, candidate).records().take(20).flatten() {
            if record.len() > 1 {
                *widths.entry(record.len()).or_default() += 1;
            }
        }
        let score = widths
            .into_iter()
            .map(|(width, count)| (count, width))
            .max()
            .unwrap_or_default();
        if score > best {
            best = score;
            selected = candidate;
        }
    }
    selected
}

fn load_csv(
    bytes: &[u8],
    tsv: bool,
    clipped: bool,
    cancel: &CancelToken,
) -> Result<TableDocument, PreviewError> {
    let text = csv_text(bytes, clipped)?;
    let mut reader = reader(&text, delimiter(&text, tsv));
    let mut sheet = TableSheet {
        name: "Data".into(),
        truncated: clipped,
        ..TableSheet::default()
    };
    let mut budget = MAX_TEXT_BYTES;
    let mut record = csv::StringRecord::new();
    while reader.read_record(&mut record).map_err(error)? {
        check(cancel)?;
        // A prefix can end inside a quoted field. Never show that partial record.
        if clipped && reader.position().byte() as usize >= text.len() {
            break;
        }
        if sheet.rows.len() == MAX_ROWS || budget == 0 {
            sheet.truncated = true;
            break;
        }
        sheet.truncated |= record.len() > MAX_COLUMNS;
        let row = record
            .iter()
            .take(MAX_COLUMNS)
            .map(|value| cell_text(value, &mut budget, &mut sheet.truncated))
            .collect::<Vec<_>>();
        sheet.columns = sheet.columns.max(row.len());
        sheet.rows.push(row);
    }
    Ok(TableDocument {
        format: if tsv { "TSV" } else { "CSV" },
        sheets: vec![sheet],
        truncated: false,
    })
}

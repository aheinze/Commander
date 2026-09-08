use std::io::{Cursor, Read};

use calamine::{Cell, CellType, Data, DataType, Reader, Xls, Xlsb, Xlsx};

use super::*;

mod legacy;

pub(super) fn load(
    bytes: &[u8],
    extension: &str,
    cancel: &CancelToken,
) -> Result<TableDocument, PreviewError> {
    if extension == "xls" {
        legacy::validate(bytes, cancel)?;
        let mut book = Xls::new(Cursor::new(bytes)).map_err(error)?;
        let names = book.sheet_names();
        let mut budget = MAX_TEXT_BYTES;
        let mut sheets = Vec::new();
        for name in names.iter().take(MAX_SHEETS) {
            check(cancel)?;
            let result = book.worksheet_range(name).map_err(error).and_then(|range| {
                let start = range.start().unwrap_or_default();
                let mut cells = range.used_cells();
                collect(
                    name,
                    start,
                    || {
                        Ok::<_, PreviewError>(cells.next().map(|(row, col, value)| {
                            Cell::new((start.0 + row as u32, start.1 + col as u32), value.clone())
                        }))
                    },
                    &mut budget,
                    cancel,
                )
            });
            sheets.push(sheet_result(name, result)?);
        }
        return Ok(TableDocument {
            format: "Excel",
            sheets,
            truncated: names.len() > MAX_SHEETS,
        });
    }
    validate_archive(bytes, cancel)?;
    let mut budget = MAX_TEXT_BYTES;
    let mut sheets = Vec::new();
    let names;
    if extension == "xlsb" {
        let mut book = Xlsb::new(Cursor::new(bytes)).map_err(error)?;
        names = book.sheet_names();
        for name in names.iter().take(MAX_SHEETS) {
            check(cancel)?;
            let result = book
                .worksheet_cells_reader(name)
                .map_err(error)
                .and_then(|mut reader| {
                    collect(
                        name,
                        reader.dimensions().start,
                        || reader.next_cell(),
                        &mut budget,
                        cancel,
                    )
                });
            sheets.push(sheet_result(name, result)?);
        }
    } else {
        let mut book = Xlsx::new(Cursor::new(bytes)).map_err(error)?;
        names = book.sheet_names();
        for name in names.iter().take(MAX_SHEETS) {
            check(cancel)?;
            let result = book
                .worksheet_cells_reader(name)
                .map_err(error)
                .and_then(|mut reader| {
                    collect(
                        name,
                        reader.dimensions().start,
                        || reader.next_cell(),
                        &mut budget,
                        cancel,
                    )
                });
            sheets.push(sheet_result(name, result)?);
        }
    }
    Ok(TableDocument {
        format: "Excel",
        sheets,
        truncated: names.len() > MAX_SHEETS,
    })
}

fn sheet_result(
    name: &str,
    result: Result<TableSheet, PreviewError>,
) -> Result<TableSheet, PreviewError> {
    match result {
        Err(PreviewError::Cancelled) => Err(PreviewError::Cancelled),
        Err(failure) => Ok(TableSheet {
            name: name.replace('\0', "�"),
            error: Some(failure.to_string()),
            ..TableSheet::default()
        }),
        Ok(sheet) => Ok(sheet),
    }
}

fn collect<T: CellType + Into<Data>, E: std::fmt::Display>(
    name: &str,
    start: (u32, u32),
    mut next: impl FnMut() -> Result<Option<Cell<T>>, E>,
    budget: &mut usize,
    cancel: &CancelToken,
) -> Result<TableSheet, PreviewError> {
    let mut sheet = TableSheet {
        name: name.replace('\0', "�"),
        first_row: start.0,
        first_column: start.1,
        ..TableSheet::default()
    };
    let mut inspected = 0;
    loop {
        check(cancel)?;
        let Some(cell) = next().map_err(error)? else {
            break;
        };
        inspected += 1;
        if inspected > 100_000 || *budget == 0 {
            sheet.truncated = true;
            break;
        }
        let (row, col) = cell.get_position();
        let Some(row) = row.checked_sub(start.0) else {
            continue;
        };
        let Some(col) = col.checked_sub(start.1) else {
            continue;
        };
        if row as usize >= MAX_ROWS {
            sheet.truncated = true;
            break;
        }
        if col as usize >= MAX_COLUMNS {
            sheet.truncated = true;
            continue;
        }
        let value: Data = cell.get_value().clone().into();
        if value.is_empty() {
            continue;
        }
        let text = format_cell(&value);
        let text = cell_text(&text, budget, &mut sheet.truncated);
        sheet
            .rows
            .resize_with(sheet.rows.len().max(row as usize + 1), Vec::new);
        let cells = &mut sheet.rows[row as usize];
        cells.resize(cells.len().max(col as usize + 1), String::new());
        cells[col as usize] = text;
        sheet.columns = sheet.columns.max(col as usize + 1);
    }
    Ok(sheet)
}

fn format_cell(value: &(impl DataType + std::fmt::Display)) -> String {
    if let Some(date) = value.get_datetime() {
        if date.is_duration() {
            let seconds = (date.as_f64() * 86400.0).round() as i64;
            let total = seconds.unsigned_abs();
            return format!(
                "{}{}:{:02}:{:02}",
                if seconds < 0 { "-" } else { "" },
                total / 3600,
                (total / 60) % 60,
                total % 60
            );
        }
        if let Some(date) = date.as_datetime() {
            return if date.time() == Default::default() {
                date.date().to_string()
            } else {
                date.to_string()
            };
        }
    }
    value.to_string()
}

// Check actual decompression before calamine reads shared strings/styles. Merely
// bounding the final table or trusting ZIP size headers does not bound that work.
fn validate_archive(bytes: &[u8], cancel: &CancelToken) -> Result<(), PreviewError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(error)?;
    if archive.len() > 16_384 {
        return Err(error("workbook contains too many parts to preview"));
    }
    let mut remaining = 128 * 1024 * 1024_u64;
    let mut buffer = [0; 32 * 1024];
    for index in 0..archive.len() {
        check(cancel)?;
        let entry = archive.by_index(index).map_err(error)?;
        let limit = remaining.min(32 * 1024 * 1024);
        if entry.size() > limit {
            return Err(error("expanded workbook exceeds the preview limit"));
        }
        let mut reader = entry.take(limit + 1);
        let mut count = 0;
        loop {
            check(cancel)?;
            let read = reader.read(&mut buffer).map_err(error)?;
            if read == 0 {
                break;
            }
            count += read as u64;
        }
        if count > limit {
            return Err(error("expanded workbook exceeds the preview limit"));
        }
        remaining -= count;
    }
    Ok(())
}

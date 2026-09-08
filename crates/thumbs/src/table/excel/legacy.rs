//! Check legacy BIFF allocation sizes before its eager workbook decoder runs.

use super::*;

pub(super) fn validate(bytes: &[u8], cancel: &CancelToken) -> Result<(), PreviewError> {
    let mut compound = cfb::CompoundFile::open(Cursor::new(bytes)).map_err(error)?;
    let name = if compound.is_stream("/Workbook") {
        "/Workbook"
    } else {
        "/Book"
    };
    let stream = compound.open_stream(name).map_err(error)?;
    if stream.len() > MAX_WORKBOOK_BYTES as u64 {
        return Err(error("legacy workbook exceeds the preview limit"));
    }
    let mut bytes = Vec::new();
    stream
        .take(MAX_WORKBOOK_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    let mut offset = 0;
    let mut total_area = 0_u64;
    let mut range = None;
    let mut reserved = 0;
    let mut sheets = Vec::new();
    let mut starts = std::collections::BTreeSet::new();
    while offset + 4 <= bytes.len() {
        check(cancel)?;
        let id = u16_at(&bytes[offset..], 0)?;
        let size = u16_at(&bytes[offset..], 2)? as usize;
        if matches!(id, 0x0809 | 0x0409 | 0x0209 | 0x0009) {
            starts.insert(offset);
        }
        offset += 4;
        let data = bytes
            .get(offset..offset + size)
            .ok_or_else(|| error("invalid legacy workbook record"))?;
        offset += size;
        match id {
            0x0085 => {
                sheets.push(u32_at(data, 0)? as usize);
                if sheets.len() > 256 {
                    return Err(error("legacy workbook contains too many sheets to preview"));
                }
            }
            0x0200 => {
                let (first_row, end_row, first_col, end_col) = match data.len() {
                    10 => (
                        u16_at(data, 0)? as u32,
                        u16_at(data, 2)? as u32,
                        u16_at(data, 4)? as u32,
                        u16_at(data, 6)? as u32,
                    ),
                    14 => (
                        u32_at(data, 0)?,
                        u32_at(data, 4)?,
                        u16_at(data, 8)? as u32,
                        u16_at(data, 10)? as u32,
                    ),
                    _ => return Err(error("invalid legacy worksheet dimensions")),
                };
                if end_row < first_row || end_col < first_col || end_row > 65_536 || end_col > 256 {
                    return Err(error("invalid legacy worksheet dimensions"));
                }
                reserved +=
                    u64::from(end_row - first_row).max(1) * u64::from(end_col - first_col).max(1);
            }
            0x0203 | 0x0204 | 0x00d6 | 0x0205 | 0x027e | 0x00fd | 0x0006 | 0x00bd => {
                let row = u16_at(data, 0)? as u32;
                let col = if id == 0x00bd {
                    u16_at(data, data.len().saturating_sub(2))?
                } else {
                    u16_at(data, 2)?
                } as u32;
                if col >= 256 {
                    return Err(error("invalid legacy worksheet column"));
                }
                let first_col = u16_at(data, 2)? as u32;
                if first_col > col {
                    return Err(error("invalid legacy worksheet columns"));
                }
                let bounds = range.get_or_insert((row, row, first_col, col));
                bounds.0 = bounds.0.min(row);
                bounds.1 = bounds.1.max(row);
                bounds.2 = bounds.2.min(first_col);
                bounds.3 = bounds.3.max(col);
            }
            0x000a => {
                total_area += area(range.take()).max(reserved);
                reserved = 0;
            }
            _ => {}
        }
        if total_area + area(range).max(reserved) > 1_000_000 {
            return Err(error(
                "legacy workbook is too large to preview; save it as XLSX",
            ));
        }
    }
    let mut unique = std::collections::BTreeSet::new();
    if sheets
        .iter()
        .any(|offset| !starts.contains(offset) || !unique.insert(*offset))
    {
        return Err(error("invalid legacy worksheet offsets"));
    }
    Ok(())
}

fn area(bounds: Option<(u32, u32, u32, u32)>) -> u64 {
    bounds.map_or(0, |(top, bottom, left, right)| {
        u64::from(bottom - top + 1) * u64::from(right - left + 1)
    })
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, PreviewError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| error("invalid legacy workbook record"))?
            .try_into()
            .unwrap(),
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, PreviewError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| error("invalid legacy workbook record"))?
            .try_into()
            .unwrap(),
    ))
}

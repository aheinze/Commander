use std::io::{Cursor, Write};

use super::*;
use dualpane_vfs::LocalFs;

fn preview(name: &str, bytes: &[u8]) -> TableDocument {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let preview = crate::load_preview(&LocalFs, VPath::from(path), &CancelToken::new()).unwrap();
    let crate::PreviewPayload::Table(table) = preview.payload else {
        panic!("spreadsheet did not become a table")
    };
    table
}

#[test]
fn csv_preserves_headers_quoted_fields_multiline_unicode_and_empty_cells() {
    let table = preview("data.CSV", b"\xef\xbb\xbfName,Code,Note\r\n\"Doe, Jane\",0012,\"Line one\nLine two\"\r\nZo\xc3\xab,,\"He said \"\"hello\"\"\"\r\nshort\r\n");
    let sheet = &table.sheets[0];
    assert_eq!(sheet.columns, 3);
    assert_eq!(sheet.rows[0], ["Name", "Code", "Note"]);
    assert_eq!(sheet.rows[1], ["Doe, Jane", "0012", "Line one\nLine two"]);
    assert_eq!(sheet.rows[2], ["Zoë", "", "He said \"hello\""]);
    assert_eq!(sheet.rows[3], ["short"]);
    assert!(!sheet.truncated);
}

#[test]
fn delimited_previews_detect_separators_and_utf16() {
    for separator in [';', '\t', '|'] {
        let source = format!("Name{separator}Amount\nA{separator}1,25\nB{separator}2,50\n");
        assert_eq!(
            preview("data.csv", source.as_bytes()).sheets[0].rows[1],
            ["A", "1,25"]
        );
    }
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for word in "City\tValue\nMünchen\t42\n".encode_utf16() {
            bytes.extend_from_slice(&if little {
                word.to_le_bytes()
            } else {
                word.to_be_bytes()
            });
        }
        assert_eq!(
            preview("data.tsv", &bytes).sheets[0].rows[1],
            ["München", "42"]
        );
    }
    assert!(preview("empty.csv", b"").sheets[0].rows.is_empty());
}

#[test]
fn table_limits_keep_complete_records_and_clip_cells_safely() {
    let row = (0..60)
        .map(|_| "é".repeat(1500))
        .collect::<Vec<_>>()
        .join(",")
        + "\n";
    let table = preview("large.csv", row.as_bytes());
    assert_eq!(table.sheets[0].columns, MAX_COLUMNS);
    assert!(table.sheets[0].truncated);
    assert!(table.sheets[0].rows[0][0].ends_with('…'));
    let table = preview("long.csv", "one,two\n".repeat(600).as_bytes());
    assert_eq!(table.sheets[0].rows.len(), MAX_ROWS);
    assert!(table.sheets[0].truncated);
    let table = load_csv(b"a,b\n1,\"partial", false, true, &CancelToken::new()).unwrap();
    assert_eq!(table.sheets[0].rows, [vec!["a", "b"]]);
    assert!(load_csv(b"a,\xff", false, false, &CancelToken::new()).is_err());
}

fn xlsx(sheet: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in [
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="xl/workbook.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Empty" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/><Relationship Id="rId2" Target="worksheets/sheet2.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#,
        ),
        (
            "xl/styles.xml",
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>"#,
        ),
        (
            "xl/sharedStrings.xml",
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>Revenue &amp; costs</t></si></sst>"#,
        ),
        ("xl/worksheets/sheet1.xml", sheet),
        (
            "xl/worksheets/sheet2.xml",
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#,
        ),
    ] {
        zip.start_file(name, zip::write::FileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn excel_previews_sheets_strings_dates_cached_formulas_and_sparse_cells() {
    let bytes = xlsx(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="B3:D5"/><sheetData><row r="3"><c r="B3" t="s"><v>0</v></c><c r="C3" s="1"><v>45292</v></c><c r="D3"><f>2+3</f><v>5</v></c></row><row r="5"><c r="D5" t="b"><v>1</v></c></row></sheetData></worksheet>"#,
    );
    for extension in ["xlsx", "xlsm"] {
        let table = preview(&format!("book.{extension}"), &bytes);
        assert_eq!(table.sheets.len(), 2);
        let sheet = &table.sheets[0];
        assert_eq!((sheet.first_row, sheet.first_column), (2, 1));
        assert_eq!(sheet.rows[0], ["Revenue & costs", "2024-01-01", "5"]);
        assert!(sheet.rows[1].is_empty());
        assert_eq!(sheet.rows[2], ["", "", "true"]);
        assert!(table.sheets[1].rows.is_empty());
    }
}

#[test]
fn excel_does_not_allocate_declared_whole_sheet_and_cancels() {
    let bytes = xlsx(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:XFD1048576"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row><row r="1048576"><c r="XFD1048576"><v>2</v></c></row></sheetData></worksheet>"#,
    );
    let table = preview("sparse.xlsx", &bytes);
    assert_eq!(table.sheets[0].rows, [vec!["1"]]);
    assert!(table.sheets[0].truncated);
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        excel::load(&bytes, "xlsx", &cancel),
        Err(PreviewError::Cancelled)
    ));
    assert!(excel::load(b"broken", "xlsx", &CancelToken::new()).is_err());
}

fn legacy_xls(huge: bool) -> Vec<u8> {
    fn record(id: u16, value: &[u8]) -> Vec<u8> {
        [
            id.to_le_bytes().as_slice(),
            (value.len() as u16).to_le_bytes().as_slice(),
            value,
        ]
        .concat()
    }
    let bof = [0x00, 0x06, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00];
    let mut global = record(0x0809, &bof);
    let offset = global.len() + 4 + 12 + 4;
    let mut bound = (offset as u32).to_le_bytes().to_vec();
    bound.extend_from_slice(&[0, 0, 4, 0]);
    bound.extend_from_slice(b"Data");
    global.extend(record(0x0085, &bound));
    global.extend(record(0x000a, &[]));
    let mut sheet_bof = bof;
    sheet_bof[2] = 0x10;
    global.extend(record(0x0809, &sheet_bof));
    let mut dimensions = vec![0; 14];
    dimensions[4..8].copy_from_slice(&if huge { 65_536_u32 } else { 1 }.to_le_bytes());
    dimensions[10..12].copy_from_slice(&if huge { 256_u16 } else { 1 }.to_le_bytes());
    global.extend(record(0x0200, &dimensions));
    let mut number = vec![0; 6];
    number.extend_from_slice(&42_f64.to_le_bytes());
    global.extend(record(0x0203, &number));
    global.extend(record(0x000a, &[]));
    // Pad beyond the CFB mini-stream threshold, matching ordinary Excel workbooks.
    global.resize(4096, 0);
    let mut compound =
        cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).unwrap();
    compound
        .create_stream("/Workbook")
        .unwrap()
        .write_all(&global)
        .unwrap();
    compound.into_inner().into_inner()
}

#[test]
fn legacy_excel_renders_and_rejects_excessive_dense_allocations() {
    assert_eq!(
        preview("legacy.xls", &legacy_xls(false)).sheets[0].rows,
        [vec!["42"]]
    );
    assert!(excel::load(&legacy_xls(true), "xls", &CancelToken::new()).is_err());
}

#[test]
fn binary_excel_cells_reach_the_table_preview() {
    fn record(id: u16, body: &[u8]) -> Vec<u8> {
        let mut record = if id < 128 {
            vec![id as u8]
        } else {
            vec![(id as u8 & 127) | 128, (id >> 7) as u8]
        };
        assert!(body.len() < 128);
        record.push(body.len() as u8);
        record.extend_from_slice(body);
        record
    }
    fn wide(value: &str) -> Vec<u8> {
        let mut bytes = (value.encode_utf16().count() as u32).to_le_bytes().to_vec();
        for word in value.encode_utf16() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }
    let mut bundle = vec![0; 8];
    bundle.extend(wide("rId1"));
    bundle.extend(wide("Data"));
    // BrtEndBundleShs leaves its empty body length to the following read_type.
    let workbook = [
        record(0x009c, &bundle),
        record(0x0090, &[]),
        record(0x0084, &[]),
    ]
    .concat();
    let mut cell = vec![0; 8];
    cell.extend_from_slice(&42_f64.to_le_bytes());
    let sheet = [
        record(0x0094, &[0; 16]),
        record(0x0091, &[]),
        record(0x0000, &[0; 4]),
        record(0x0005, &cell),
        record(0x0092, &[]),
    ]
    .concat();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("xl/workbook.bin", workbook.as_slice()),
        ("xl/worksheets/sheet1.bin", sheet.as_slice()),
        ("xl/_rels/workbook.bin.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.bin" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#.as_slice()),
    ] {
        zip.start_file(name, zip::write::FileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    let table = preview("book.xlsb", &zip.finish().unwrap().into_inner());
    assert_eq!(table.sheets[0].name, "Data");
    assert_eq!(table.sheets[0].rows, [vec!["42"]]);
}

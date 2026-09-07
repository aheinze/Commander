use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::Path;

use dualpane_core::{CancelToken, SizeHint, VPath};
use dualpane_vfs::Vfs;
use lopdf::{Document, Object, ObjectId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdfTool {
    Compress,
    Merge,
    ExtractPages,
    SplitPages,
    RotatePages,
    Unlock,
}

#[derive(Clone, Debug)]
pub struct PdfToolOptions {
    pub tool: PdfTool,
    pub page_ranges: String,
    pub rotation: i64,
    pub password: String,
}

pub fn run_pdf_tool(
    vfs: &dyn Vfs,
    sources: &[VPath],
    destination: &VPath,
    options: &PdfToolOptions,
    cancel: &CancelToken,
) -> Result<Vec<VPath>, String> {
    if sources.is_empty() {
        return Err("Select at least one PDF".to_owned());
    }
    if options.tool == PdfTool::Merge {
        if sources.len() < 2 {
            return Err("Select at least two PDFs to merge".to_owned());
        }
        let mut documents = Vec::with_capacity(sources.len());
        for source in sources {
            documents.push(load_document(vfs, source, None, cancel)?);
        }
        let output = unique_pdf_path(vfs, destination, "Merged.pdf");
        save_document(vfs, merge_documents(documents)?, &output)?;
        return Ok(vec![output]);
    }

    let mut outputs = Vec::new();
    for source in sources {
        cancel
            .check()
            .map_err(|_| "PDF operation cancelled".to_owned())?;
        let stem = source
            .file_name()
            .and_then(|name| Path::new(name).file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("Document");
        match options.tool {
            PdfTool::Compress => {
                let mut document = load_document(vfs, source, None, cancel)?;
                document.compress();
                let output = unique_pdf_path(vfs, destination, &format!("{stem} compressed.pdf"));
                save_document(vfs, document, &output)?;
                outputs.push(output);
            }
            PdfTool::ExtractPages => {
                let mut document = load_document(vfs, source, None, cancel)?;
                let pages = parse_page_ranges(
                    &options.page_ranges,
                    u32::try_from(document.get_pages().len()).unwrap_or(u32::MAX),
                    false,
                )?;
                keep_pages(&mut document, &pages);
                let output = unique_pdf_path(vfs, destination, &format!("{stem} extracted.pdf"));
                save_document(vfs, document, &output)?;
                outputs.push(output);
            }
            PdfTool::SplitPages => {
                let document = load_document(vfs, source, None, cancel)?;
                let count = u32::try_from(document.get_pages().len()).unwrap_or(u32::MAX);
                for page in 1..=count {
                    cancel
                        .check()
                        .map_err(|_| "PDF operation cancelled".to_owned())?;
                    let mut page_document = load_document(vfs, source, None, cancel)?;
                    keep_pages(&mut page_document, &[page]);
                    let output =
                        unique_pdf_path(vfs, destination, &format!("{stem} page {page:02}.pdf"));
                    save_document(vfs, page_document, &output)?;
                    outputs.push(output);
                }
            }
            PdfTool::RotatePages => {
                let mut document = load_document(vfs, source, None, cancel)?;
                let count = u32::try_from(document.get_pages().len()).unwrap_or(u32::MAX);
                let pages = parse_page_ranges(&options.page_ranges, count, true)?;
                rotate_pages(&mut document, &pages, options.rotation)?;
                let output = unique_pdf_path(vfs, destination, &format!("{stem} rotated.pdf"));
                save_document(vfs, document, &output)?;
                outputs.push(output);
            }
            PdfTool::Unlock => {
                let document = load_document(vfs, source, Some(&options.password), cancel)?;
                let output = unique_pdf_path(vfs, destination, &format!("{stem} unlocked.pdf"));
                save_document(vfs, document, &output)?;
                outputs.push(output);
            }
            PdfTool::Merge => unreachable!("merge is handled before the source loop"),
        }
    }
    Ok(outputs)
}

fn load_document(
    vfs: &dyn Vfs,
    path: &VPath,
    password: Option<&str>,
    cancel: &CancelToken,
) -> Result<Document, String> {
    let mut reader = vfs.open_read(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read {path}: {error}"))?;
    cancel
        .check()
        .map_err(|_| "PDF operation cancelled".to_owned())?;
    let mut document = Document::load_mem(&bytes)
        .map_err(|error| format!("Could not parse PDF {path}: {error}"))?;
    if document.is_encrypted() {
        let Some(password) = password else {
            return Err(format!("{path} is encrypted; unlock it first"));
        };
        document
            .decrypt(password)
            .map_err(|error| format!("Could not unlock {path}: {error}"))?;
    }
    Ok(document)
}

fn save_document(vfs: &dyn Vfs, mut document: Document, output: &VPath) -> Result<(), String> {
    document.prune_objects();
    document.renumber_objects();
    let mut writer = vfs
        .create_write(output, SizeHint::Unknown)
        .map_err(|error| format!("Could not create {output}: {error}"))?;
    document
        .save_to(&mut writer)
        .map_err(|error| format!("Could not save {output}: {error}"))?;
    Ok(())
}

fn merge_documents(documents: Vec<Document>) -> Result<Document, String> {
    let mut max_id = 1;
    let mut pages = Vec::<(ObjectId, Object)>::new();
    let mut objects = BTreeMap::<ObjectId, Object>::new();
    let mut output = Document::with_version("1.5");
    for mut document in documents {
        document.renumber_objects_with(max_id);
        max_id = document.max_id + 1;
        for page_id in document.get_pages().into_values() {
            pages.push((
                page_id,
                document
                    .get_object(page_id)
                    .map_err(|error| format!("Could not read PDF page: {error}"))?
                    .to_owned(),
            ));
        }
        objects.extend(document.objects);
    }
    let mut catalog: Option<(ObjectId, Object)> = None;
    let mut page_tree: Option<(ObjectId, Object)> = None;
    for (id, object) in objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" => catalog = Some((catalog.map_or(id, |value| value.0), object)),
            b"Pages" => {
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();
                    if let Some((_, existing)) = &page_tree
                        && let Ok(existing) = existing.as_dict()
                    {
                        dictionary.extend(existing);
                    }
                    page_tree = Some((
                        page_tree.map_or(id, |value| value.0),
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            b"Page" | b"Outlines" | b"Outline" => {}
            _ => {
                output.objects.insert(id, object);
            }
        }
    }
    let (pages_id, pages_object) = page_tree.ok_or("PDF page tree is missing")?;
    let (catalog_id, catalog_object) = catalog.ok_or("PDF catalog is missing")?;
    for (id, object) in &pages {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_id);
            output.objects.insert(*id, Object::Dictionary(dictionary));
        }
    }
    if let Ok(dictionary) = pages_object.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Count", pages.len() as u32);
        dictionary.set(
            "Kids",
            pages
                .iter()
                .map(|(id, _)| Object::Reference(*id))
                .collect::<Vec<_>>(),
        );
        output
            .objects
            .insert(pages_id, Object::Dictionary(dictionary));
    }
    if let Ok(dictionary) = catalog_object.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", pages_id);
        dictionary.remove(b"Outlines");
        output
            .objects
            .insert(catalog_id, Object::Dictionary(dictionary));
    }
    output.trailer.set("Root", catalog_id);
    output.max_id = output.objects.keys().map(|id| id.0).max().unwrap_or(0);
    output.renumber_objects();
    output.adjust_zero_pages();
    Ok(output)
}

fn keep_pages(document: &mut Document, selected_pages: &[u32]) {
    let selected = selected_pages.iter().copied().collect::<HashSet<_>>();
    let delete = document
        .get_pages()
        .keys()
        .copied()
        .filter(|page| !selected.contains(page))
        .collect::<Vec<_>>();
    document.delete_pages(&delete);
}

fn rotate_pages(document: &mut Document, pages: &[u32], rotation: i64) -> Result<(), String> {
    let rotation = match rotation.rem_euclid(360) {
        value @ (90 | 180 | 270) => value,
        _ => return Err("Rotation must be 90, 180, or 270 degrees".to_owned()),
    };
    let page_map = document.get_pages();
    for page in pages {
        let Some(id) = page_map.get(page) else {
            continue;
        };
        let dictionary = document
            .get_object_mut(*id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| format!("Could not rotate page {page}: {error}"))?;
        let current = dictionary
            .get(b"Rotate")
            .and_then(Object::as_i64)
            .unwrap_or(0);
        dictionary.set("Rotate", (current + rotation).rem_euclid(360));
    }
    Ok(())
}

fn parse_page_ranges(
    input: &str,
    page_count: u32,
    allow_empty_all: bool,
) -> Result<Vec<u32>, String> {
    let input = input.trim();
    if input.is_empty() && allow_empty_all {
        return Ok((1..=page_count).collect());
    }
    if input.is_empty() {
        return Err("Enter pages such as 1-3,5".to_owned());
    }
    let mut pages = Vec::new();
    let mut seen = HashSet::new();
    for token in input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let (start, end) = token.split_once('-').unwrap_or((token, token));
        let start = parse_page(start)?;
        let end = parse_page(end)?;
        if start > end || end > page_count {
            return Err(format!(
                "Page range {token} is outside this {page_count}-page PDF"
            ));
        }
        for page in start..=end {
            if seen.insert(page) {
                pages.push(page);
            }
        }
    }
    Ok(pages)
}

fn parse_page(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Invalid PDF page number: {value}"))
}

fn unique_pdf_path(vfs: &dyn Vfs, directory: &VPath, name: &str) -> VPath {
    let initial = directory.join_name(name.as_ref());
    if vfs.stat(&initial, false).is_err() {
        return initial;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Document");
    for index in 2..10_000 {
        let candidate = directory.join_name(format!("{stem} {index}.pdf").as_ref());
        if vfs.stat(&candidate, false).is_err() {
            return candidate;
        }
    }
    directory.join_name(format!("{stem} copy.pdf").as_ref())
}

#[cfg(test)]
mod tests {
    use super::parse_page_ranges;

    #[test]
    fn parses_pdf_page_ranges() {
        assert_eq!(
            parse_page_ranges("1-3,5", 6, false).unwrap(),
            vec![1, 2, 3, 5]
        );
        assert!(parse_page_ranges("7", 6, false).is_err());
    }
}

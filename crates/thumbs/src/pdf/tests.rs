use super::*;
use lopdf::{Document, Object, dictionary};

fn fixture() -> Vec<u8> {
    let mut pdf = Document::with_version("1.4");
    let parent = pdf.new_object_id();
    let mut pages = Vec::new();
    for (width, height, rotation) in [(300, 600, 0), (600, 300, 90), (800, 400, 0)] {
        pages.push(pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => parent,
            "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
            "Rotate" => rotation,
        }));
    }
    pdf.objects.insert(
        parent,
        dictionary! {
            "Type" => "Pages", "Count" => 3,
            "Kids" => pages.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>()
        }
        .into(),
    );
    let outline = pdf.new_object_id();
    let first = pdf.new_object_id();
    pdf.objects.insert(
        first,
        dictionary! {
            "Title" => Object::string_literal("Chapter two"), "Parent" => outline,
            "Dest" => vec![Object::Reference(pages[1]), Object::Name(b"Fit".to_vec())],
            "Next" => first, // Malformed cycle must terminate.
        }
        .into(),
    );
    pdf.objects.insert(
        outline,
        dictionary! { "First" => first, "Last" => first, "Count" => 1 }.into(),
    );
    let root = pdf
        .add_object(dictionary! { "Type" => "Catalog", "Pages" => parent, "Outlines" => outline });
    pdf.trailer.set("Root", root);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    bytes
}

#[test]
fn pdf_document_preserves_rotation_and_terminates_cyclic_bookmarks() {
    let document = PdfDocument::load(fixture(), &CancelToken::new()).unwrap();
    assert_eq!(document.pages.len(), 3);
    assert_eq!(document.pages[1].width, 300.0);
    assert_eq!(document.pages[1].height, 600.0);
    assert_eq!(document.bookmarks.len(), 1);
    assert_eq!(document.bookmarks[0].page, Some(1));
    assert_eq!(document.bookmarks[0].title, "Chapter two");
}

#[test]
fn pdf_bookmarks_resolve_named_destinations_and_local_actions() {
    let mut pdf = Document::load_mem(&fixture()).unwrap();
    let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let outline = pdf
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    let first = pdf
        .get_dictionary(outline)
        .unwrap()
        .get(b"First")
        .unwrap()
        .as_reference()
        .unwrap();
    let page = pdf.get_pages()[&3];
    let names = pdf.add_object(dictionary! { "Names" => vec![
        Object::string_literal("chapter-three"),
        Object::Dictionary(dictionary! { "D" => vec![Object::Reference(page), Object::Name(b"Fit".to_vec())] })
    ] });
    pdf.get_object_mut(root)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Names", dictionary! { "Dests" => names });
    let bookmark = pdf.get_object_mut(first).unwrap().as_dict_mut().unwrap();
    bookmark.remove(b"Dest");
    bookmark.set(
        "A",
        dictionary! { "S" => "GoTo", "D" => Object::string_literal("chapter-three") },
    );
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let document = PdfDocument::load(bytes, &CancelToken::new()).unwrap();
    assert_eq!(document.bookmarks[0].page, Some(2));
}

#[test]
fn pdf_worker_serves_independent_viewers_and_survives_cancelled_batches() {
    let document = PdfDocument::load(fixture(), &CancelToken::new()).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = document.request(vec![(0, 1.0)], cancel).unwrap();
    assert!(cancelled.recv_timeout(Duration::from_secs(5)).is_err());
    let first = document
        .request(vec![(0, 0.2), (2, 0.2)], CancelToken::new())
        .unwrap();
    let second = document
        .request(vec![(1, 0.5), (999, 0.5)], CancelToken::new())
        .unwrap();
    let page = first.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(page.index, 0);
    let raster = page.result.unwrap();
    assert_eq!((raster.width, raster.height), (60, 120));
    assert_eq!(raster.rgba.len(), 60 * 120 * 4);
    assert_eq!(
        first.recv_timeout(Duration::from_secs(10)).unwrap().index,
        2
    );
    let page = second.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(page.index, 1);
    assert_eq!(page.result.unwrap().height, 300);
    assert!(
        second
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .result
            .is_err()
    );
}

#[test]
fn pdf_raster_limits_apply_before_allocating_even_for_extreme_zoom() {
    for size in [
        PageSize {
            width: 14400.0,
            height: 14400.0,
        },
        PageSize {
            width: 1.0,
            height: 14400.0,
        },
    ] {
        for requested in [f32::NAN, f32::INFINITY, -1.0, 0.01, 5.0, 999.0] {
            let scale = raster_scale(size, requested);
            assert!(scale.is_finite() && scale > 0.0);
            assert!(size.width.max(size.height) * scale <= MAX_EDGE + 1.0);
            assert!(size.width * size.height * scale * scale <= MAX_PIXELS + 1.0);
        }
    }
}

#[test]
fn pdf_load_rejects_invalid_data_and_pre_cancelled_work() {
    assert!(PdfDocument::load(b"not a PDF".to_vec(), &CancelToken::new()).is_err());
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        PdfDocument::load(fixture(), &cancel),
        Err(PreviewError::Cancelled)
    ));
    assert_eq!(decode_title(&[0xfe, 0xff, 0x00, 0xdc, 0x00, 0x62]), "Üb");
    assert_eq!(decode_title(&[b'A', 0x85, b'B']), "A–B");
}

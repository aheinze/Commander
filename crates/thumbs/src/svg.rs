//! Static SVG rendering on the preview worker, without external resource access.

use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use resvg::{tiny_skia, usvg};

use super::{CancelToken, MAX_IMAGE_EDGE, PreviewError, PreviewPayload, VPath, Vfs};

const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;
const MAX_EMBEDDED_PIXELS: u64 = 16 * 1024 * 1024;

pub(super) fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

pub(super) fn load(
    vfs: &dyn Vfs,
    path: &VPath,
    cancel: &CancelToken,
) -> Result<PreviewPayload, PreviewError> {
    let mut bytes = Vec::new();
    vfs.open_read(path)?
        .take((MAX_SVG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    if bytes.len() > MAX_SVG_BYTES {
        return Err(PreviewError::Svg(
            "file exceeds the 8 MiB preview limit".into(),
        ));
    }
    let source =
        std::str::from_utf8(&bytes).map_err(|_| PreviewError::Svg("invalid SVG text".into()))?;
    render(source, MAX_IMAGE_EDGE, cancel)
}

fn render(
    source: &str,
    max_edge: u32,
    cancel: &CancelToken,
) -> Result<PreviewPayload, PreviewError> {
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS
        .get_or_init(|| {
            let mut fonts = usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone();
    let data_resolver = usvg::ImageHrefResolver::default_data_resolver();
    let options = usvg::Options {
        font_family: "sans-serif".into(),
        fontdb: fonts,
        image_href_resolver: usvg::ImageHrefResolver {
            // A selected SVG must not load arbitrary files or remote resources.
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(move |mime, data, options| {
                // Bound embedded raster decoding; nested SVG/data recursion is omitted.
                if data.len() > MAX_SVG_BYTES {
                    return None;
                }
                let (width, height) = image::ImageReader::new(Cursor::new(data.as_slice()))
                    .with_guessed_format()
                    .ok()?
                    .into_dimensions()
                    .ok()?;
                if u64::from(width) * u64::from(height) > MAX_EMBEDDED_PIXELS {
                    return None;
                }
                data_resolver(mime, data, options)
            }),
        },
        ..usvg::Options::default()
    };
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    let tree = usvg::Tree::from_str(source, &options)
        .map_err(|error| PreviewError::Svg(error.to_string()))?;
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    let size = tree.size();
    // Render vectors at preview resolution, including small icons, so enlarging
    // them in Quick Look does not magnify a tiny rasterization.
    let scale = max_edge as f32 / size.width().max(size.height());
    if !scale.is_finite() {
        return Err(PreviewError::Svg("invalid image dimensions".into()));
    }
    let width = (size.width() * scale).round().clamp(1.0, max_edge as f32) as u32;
    let height = (size.height() * scale).round().clamp(1.0, max_edge as f32) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| PreviewError::Svg("invalid image dimensions".into()))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    // The shared GTK image path expects straight RGBA, while tiny-skia emits
    // premultiplied colors. Preserve semitransparent colors when converting.
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    cancel.check().map_err(|_| PreviewError::Cancelled)?;
    Ok(PreviewPayload::Image {
        rgba,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualpane_vfs::LocalFs;

    #[test]
    fn svg_selection_produces_an_image_with_correct_alpha_and_aspect_ratio() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("drawing.SVG");
        std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"><rect width="10" height="10" fill="red" opacity="0.5"/></svg>"#).unwrap();
        let preview =
            crate::load_preview(&LocalFs, VPath::from(path), &CancelToken::new()).unwrap();
        let PreviewPayload::Image {
            rgba,
            width,
            height,
        } = preview.payload
        else {
            panic!("SVG fell through to source text")
        };
        assert_eq!((width, height), (1600, 800));
        assert_eq!(&rgba[..4], &[255, 0, 0, 128]);
        assert_eq!(
            &rgba[(width as usize - 1) * 4..width as usize * 4],
            &[0, 0, 0, 0]
        );
    }

    #[test]
    fn invalid_oversized_and_cancelled_svg_previews_fail_cleanly() {
        assert!(render("<svg broken", 64, &CancelToken::new()).is_err());
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            render("<svg/>", 64, &cancel),
            Err(PreviewError::Cancelled)
        ));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.svg");
        std::fs::write(&path, vec![b' '; MAX_SVG_BYTES + 1]).unwrap();
        assert!(matches!(
            load(&LocalFs, &VPath::from(path), &CancelToken::new()),
            Err(PreviewError::Svg(_))
        ));
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000000" height="1000000"><rect width="1000000" height="1000000" fill="red"/></svg>"#;
        let PreviewPayload::Image {
            width,
            height,
            rgba,
        } = render(source, 64, &CancelToken::new()).unwrap()
        else {
            panic!()
        };
        assert_eq!((width, height, rgba.len()), (64, 64, 64 * 64 * 4));
    }

    #[test]
    fn svg_does_not_load_referenced_local_images() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("private.png");
        image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]))
            .save(&image)
            .unwrap();
        let source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image xlink:href="{}" width="10" height="10"/></svg>"#,
            image.display()
        );
        let PreviewPayload::Image { rgba, .. } = render(&source, 64, &CancelToken::new()).unwrap()
        else {
            panic!()
        };
        assert!(rgba.iter().all(|byte| *byte == 0));
    }
}

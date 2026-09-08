//! Preserve the declared GTK 4.14 baseline, before CSS variables and color-mix.
use regex::{Captures, Regex};

pub(super) fn legacy_stylesheet(css: &str) -> String {
    let root = Regex::new(r"(?s):root\s*\{([^}]*)\}").unwrap();
    let property = Regex::new(r"--([a-z-]+)\s*:\s*([^;]+);").unwrap();
    let css = root.replace_all(css, |capture: &Captures<'_>| {
        property
            .captures_iter(&capture[1])
            .map(|capture| {
                format!(
                    "@define-color {} {};\n",
                    capture[1].replace('-', "_"),
                    &capture[2]
                )
            })
            .collect::<String>()
    });
    let variable = Regex::new(r"var\(--([a-z-]+)\)").unwrap();
    let css = variable.replace_all(&css, |capture: &Captures<'_>| {
        format!("@{}", capture[1].replace('-', "_"))
    });
    let mix = Regex::new(r"color-mix\(in srgb, ([^,]+) ([0-9]+)%, (transparent|white|@[a-z_]+)\)")
        .unwrap();
    let mut css = mix
        .replace_all(&css, |capture: &Captures<'_>| {
            let weight = capture[2].parse::<f64>().unwrap() / 100.0;
            if &capture[3] == "transparent" {
                format!("alpha({}, {weight:.2})", &capture[1])
            } else {
                format!("mix({}, {}, {:.2})", &capture[1], &capture[3], 1.0 - weight)
            }
        })
        .into_owned();
    // GTK 4.14's theme uses negative handle margins for a wider drag target.
    // The app's global separator reset removes its compensating borders.
    css.push_str("\npaned.horizontal > separator { border-left: 4px solid transparent; border-right: 4px solid transparent; background-clip: content-box; }\npaned.vertical > separator { border-top: 4px solid transparent; border-bottom: 4px solid transparent; background-clip: content-box; }\n");
    css
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::*;

    #[test]
    fn legacy_styles_cover_every_modern_expression_in_the_bundled_theme() {
        let css = legacy_stylesheet(include_str!("../../../assets/style.css"));
        assert!(!css.contains("var("));
        assert!(!css.contains("color-mix("));
        assert!(!css.contains(":root {"));
        assert!(css.contains("@define-color window_bg_color @carelo_pane;"));
        assert!(css.contains("alpha(currentColor, 0.70)"));
        assert!(css.contains("mix(@accent_bg_color, white, 0.12)"));
        // Keep named-color conversion covered even when no component uses this mix.
        let named_mix = legacy_stylesheet(
            ".example { color: color-mix(in srgb, var(--dialog-fg-color) 72%, var(--dialog-bg-color)); }",
        );
        assert!(named_mix.contains("mix(@dialog_fg_color, @dialog_bg_color, 0.28)"));
    }

    #[test]
    #[ignore = "requires a GTK display; run in the isolated native suite"]
    fn gtk_current_and_legacy_styles_parse_without_errors() {
        adw::init().unwrap();
        let bundled = include_str!("../../../assets/style.css");
        let legacy = legacy_stylesheet(bundled);
        let styles = if gtk::minor_version() < 16 {
            vec![legacy.as_str()]
        } else {
            vec![bundled, legacy.as_str()]
        };
        for css in styles {
            let provider = gtk::CssProvider::new();
            let errors = Rc::new(RefCell::new(Vec::new()));
            {
                let errors = Rc::clone(&errors);
                provider.connect_parsing_error(move |_, _, error| {
                    errors.borrow_mut().push(error.to_string());
                });
            }
            provider.load_from_string(css);
            assert!(
                errors.borrow().is_empty(),
                "CSS parser errors: {:?}",
                errors.borrow()
            );
        }
    }
}

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

const PALETTE_RELATIVE_PATH: &str = "omarchy/current/theme/colors.toml";
const CURRENT_RELATIVE_PATH: &str = "omarchy/current";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OmarchyPalette {
    mode: PaletteMode,
    accent: HexColor,
    selection: HexColor,
    muted: HexColor,
    background: HexColor,
    dark_background: HexColor,
    darker_background: HexColor,
    lighter_background: HexColor,
    foreground: HexColor,
    dark_foreground: HexColor,
    light_foreground: HexColor,
    bright_foreground: HexColor,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum PaletteMode {
    Dark,
    Light,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HexColor(String);

#[derive(Debug, Deserialize)]
struct PaletteFile {
    mode: PaletteMode,
    accent: String,
    selection: String,
    muted: String,
    background: String,
    dark_background: String,
    darker_background: String,
    lighter_background: String,
    foreground: String,
    dark_foreground: String,
    light_foreground: String,
    bright_foreground: String,
}

#[derive(Debug, Error)]
pub enum OmarchyPaletteError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::de::Error,
    },
    #[error("{field} in {path} is not a CSS hex color")]
    InvalidColor { path: PathBuf, field: &'static str },
}

impl OmarchyPalette {
    pub fn gtk_css(&self) -> String {
        let mut css = String::with_capacity(1_000);
        let _ = writeln!(
            css,
            "@define-color carelo_void {};",
            self.darker_background.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_sidebar {};",
            self.dark_background.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_toolbar {};",
            self.lighter_background.css()
        );
        let _ = writeln!(css, "@define-color carelo_pane {};", self.background.css());
        let _ = writeln!(
            css,
            "@define-color carelo_footer {};",
            self.dark_background.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_control {};",
            self.lighter_background.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_hairline {};",
            self.muted.rgba(0.45)
        );
        let _ = writeln!(css, "@define-color carelo_text {};", self.foreground.css());
        let _ = writeln!(
            css,
            "@define-color carelo_muted {};",
            self.light_foreground.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_faint {};",
            self.dark_foreground.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_icon {};",
            self.light_foreground.css()
        );
        let _ = writeln!(css, "@define-color carelo_accent {};", self.accent.css());
        let _ = writeln!(
            css,
            "@define-color carelo_on_accent {};",
            self.accent.on_color()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_hover {};",
            self.foreground.rgba(0.07)
        );
        let _ = writeln!(
            css,
            "@define-color carelo_selected {};",
            self.selection.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_selected_text {};",
            self.bright_foreground.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu {};",
            self.lighter_background.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu_control {};",
            self.background.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu_border {};",
            self.muted.rgba(0.45)
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu_text {};",
            self.foreground.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu_muted {};",
            self.light_foreground.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu_faint {};",
            self.dark_foreground.css()
        );
        let _ = writeln!(
            css,
            "@define-color carelo_menu_icon {};",
            self.light_foreground.css()
        );
        css
    }

    pub const fn is_light(&self) -> bool {
        matches!(self.mode, PaletteMode::Light)
    }
}

impl HexColor {
    fn parse(value: String) -> Option<Self> {
        let hex = value.strip_prefix('#')?;
        matches!(hex.len(), 3 | 4 | 6 | 8)
            .then_some(())
            .filter(|()| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))?;
        Some(Self(format!("#{hex}")))
    }

    fn css(&self) -> &str {
        &self.0
    }

    fn rgb(&self) -> (u8, u8, u8) {
        let expanded = match self.0.len() {
            4 | 5 => self
                .0
                .chars()
                .skip(1)
                .take(3)
                .flat_map(|character| [character, character])
                .collect::<String>(),
            _ => self.0[1..7].to_owned(),
        };
        let red = u8::from_str_radix(&expanded[0..2], 16).unwrap_or_default();
        let green = u8::from_str_radix(&expanded[2..4], 16).unwrap_or_default();
        let blue = u8::from_str_radix(&expanded[4..6], 16).unwrap_or_default();
        (red, green, blue)
    }

    fn rgba(&self, alpha: f32) -> String {
        let (red, green, blue) = self.rgb();
        format!("rgba({red}, {green}, {blue}, {alpha:.2})")
    }

    /// Text colour that stays legible on a solid fill of this colour.
    ///
    /// Pastel accents (most Omarchy palettes) need dark text; saturated ones
    /// keep the conventional white. The threshold sits above WCAG's neutral
    /// point so vivid blues such as `#0a84ff` still get white text.
    fn on_color(&self) -> &'static str {
        let (red, green, blue) = self.rgb();
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let luminance = 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
        if luminance > 0.3 {
            "#101214"
        } else {
            "#ffffff"
        }
    }
}

pub fn load_current_palette() -> Result<Option<OmarchyPalette>, OmarchyPaletteError> {
    let Some(path) = current_palette_path() else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path).map_err(|source| OmarchyPaletteError::Read {
        path: path.clone(),
        source,
    })?;
    let palette: PaletteFile =
        toml_edit::de::from_str(&contents).map_err(|source| OmarchyPaletteError::Parse {
            path: path.clone(),
            source,
        })?;
    palette.try_into_palette(path).map(Some)
}

pub fn current_theme_directory() -> Option<PathBuf> {
    let path = state_home()?.join(CURRENT_RELATIVE_PATH);
    path.is_dir().then_some(path)
}

pub fn current_theme_name() -> Option<String> {
    let name = fs::read_to_string(state_home()?.join("omarchy/current/theme.name")).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| title_case_slug(name))
}

fn current_palette_path() -> Option<PathBuf> {
    Some(state_home()?.join(PALETTE_RELATIVE_PATH))
}

fn state_home() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
}

fn title_case_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl PaletteFile {
    fn try_into_palette(self, path: PathBuf) -> Result<OmarchyPalette, OmarchyPaletteError> {
        macro_rules! color {
            ($field:ident) => {
                HexColor::parse(self.$field).ok_or_else(|| OmarchyPaletteError::InvalidColor {
                    path: path.clone(),
                    field: stringify!($field),
                })?
            };
        }
        Ok(OmarchyPalette {
            mode: self.mode,
            accent: color!(accent),
            selection: color!(selection),
            muted: color!(muted),
            background: color!(background),
            dark_background: color!(dark_background),
            darker_background: color!(darker_background),
            lighter_background: color!(lighter_background),
            foreground: color!(foreground),
            dark_foreground: color!(dark_foreground),
            light_foreground: color!(light_foreground),
            bright_foreground: color!(bright_foreground),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{HexColor, PaletteFile, PaletteMode, title_case_slug};

    #[test]
    fn parses_the_canonical_omarchy_palette() {
        let palette: PaletteFile = toml_edit::de::from_str(
            r##"
                mode = "dark"
                accent = "#89b4fa"
                selection = "#353543"
                muted = "#45475a"
                background = "#1e1e2e"
                dark_background = "#171723"
                darker_background = "#0f0f17"
                lighter_background = "#353543"
                foreground = "#cdd6f4"
                dark_foreground = "#9aa1b7"
                light_foreground = "#d5dcf6"
                bright_foreground = "#dae0f7"
            "##,
        )
        .expect("palette");
        let palette = palette
            .try_into_palette("colors.toml".into())
            .expect("valid colors");

        assert_eq!(palette.mode, PaletteMode::Dark);
        assert!(
            palette
                .gtk_css()
                .contains("@define-color carelo_accent #89b4fa;")
        );
        assert!(
            palette
                .gtk_css()
                .contains("@define-color carelo_on_accent #101214;")
        );
        assert!(
            palette
                .gtk_css()
                .contains("@define-color carelo_pane #1e1e2e;")
        );
        assert!(
            palette
                .gtk_css()
                .contains("@define-color carelo_menu #353543;")
        );
        assert!(!palette.is_light());
    }

    #[test]
    fn rejects_values_that_could_escape_generated_css() {
        assert!(HexColor::parse("#abcdef".to_owned()).is_some());
        assert!(HexColor::parse("#abc".to_owned()).is_some());
        assert!(HexColor::parse("red; } window { opacity: 0".to_owned()).is_none());
        assert!(HexColor::parse("#abcdex".to_owned()).is_none());
    }

    #[test]
    fn formats_the_current_theme_slug_for_settings() {
        assert_eq!(title_case_slug("tokyo-night"), "Tokyo Night");
        assert_eq!(title_case_slug("aether"), "Aether");
    }
}

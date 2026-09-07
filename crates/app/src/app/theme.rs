//! Appearance modes, colour themes, and Omarchy palette integration.

use super::*;

thread_local! {
    static APPEARANCE_CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static THEME_CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

pub(super) fn install_styles(appearance: AppearanceMode) {
    apply_appearance(appearance);
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../../assets/style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub(super) fn apply_appearance(appearance: AppearanceMode) {
    apply_appearance_with_palette(appearance, None);
}

pub(super) fn apply_appearance_with_palette(
    appearance: AppearanceMode,
    omarchy_light: Option<bool>,
) {
    let style_manager = adw::StyleManager::default();
    style_manager.set_color_scheme(match appearance {
        AppearanceMode::System => match omarchy_light {
            Some(true) => adw::ColorScheme::ForceLight,
            Some(false) => adw::ColorScheme::ForceDark,
            None => adw::ColorScheme::Default,
        },
        AppearanceMode::Light => adw::ColorScheme::ForceLight,
        AppearanceMode::Dark => adw::ColorScheme::ForceDark,
    });
    let light = appearance == AppearanceMode::Light
        || (appearance == AppearanceMode::System
            && omarchy_light.unwrap_or_else(|| !style_manager.is_dark()));
    // A light Omarchy palette already carries light tokens; only a dark palette
    // shown in light mode needs the built-in light set on top.
    let palette_is_light = omarchy_light == Some(true);
    APPEARANCE_CSS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                // Above the colour-theme provider so light tokens win over a dark palette.
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 3,
                );
            }
            provider
        });
        let mut css = String::new();
        if light && !palette_is_light {
            css.push_str(LIGHT_TOKENS);
        }
        if light {
            css.push_str(LIGHT_FIXES);
        }
        provider.load_from_string(&css);
    });
}

/// The built-in light palette. Every surface in `style.css` is expressed in
/// these tokens, so redefining them re-themes the whole shell, dialogs included.
const LIGHT_TOKENS: &str = r#"
@define-color carelo_void #edf0f3;
@define-color carelo_sidebar #f5f6f8;
@define-color carelo_toolbar #ffffff;
@define-color carelo_pane #ffffff;
@define-color carelo_footer #ffffff;
@define-color carelo_control #f0f2f5;
@define-color carelo_hairline rgba(25, 35, 50, 0.09);
@define-color carelo_text #252a33;
@define-color carelo_muted #59616d;
@define-color carelo_faint #6b7380;
@define-color carelo_icon #737d8c;
@define-color carelo_hover rgba(0, 0, 0, 0.045);
@define-color carelo_selected alpha(@carelo_accent, 0.16);
@define-color carelo_selected_text @carelo_text;
@define-color carelo_menu #ffffff;
@define-color carelo_menu_control #f1f3f4;
@define-color carelo_menu_border rgba(0, 0, 0, 0.12);
@define-color carelo_menu_text #1c1f24;
@define-color carelo_menu_muted #5f646b;
@define-color carelo_menu_faint rgba(28, 31, 36, 0.50);
@define-color carelo_menu_icon rgba(28, 31, 36, 0.60);
"#;

/// Semantic status colours retain contrast in the built-in light appearance.
const LIGHT_FIXES: &str = r#"
dialog-host > dimming { background: rgba(20, 22, 26, 0.24); }
.git-pill-modified { color: #995300; }
.git-pill-untracked { color: #1f7546; }
.git-pill-conflict { color: #bc3030; }
"#;

pub(super) fn apply_color_theme(theme: ColorTheme, appearance: AppearanceMode) {
    let mut omarchy_palette = None;
    let css = match theme {
        ColorTheme::Automatic => match omarchy::load_current_palette() {
            Ok(Some(palette)) => {
                let css = palette.gtk_css();
                omarchy_palette = Some(palette);
                css
            }
            Ok(None) => String::new(),
            Err(error) => {
                tracing::warn!(%error, "ignoring invalid Omarchy color palette");
                String::new()
            }
        },
        ColorTheme::Carelo => String::new(),
        ColorTheme::Midnight => r#"
            @define-color carelo_void #11151f;
            @define-color carelo_sidebar #171d2a;
            @define-color carelo_toolbar #222b3b;
            @define-color carelo_pane #1c2432;
            @define-color carelo_footer #192130;
            @define-color carelo_control #2c374a;
            @define-color carelo_accent #6ca9ff;
            @define-color carelo_on_accent #101214;
            "#
        .to_owned(),
        ColorTheme::Forest => r#"
            @define-color carelo_void #111814;
            @define-color carelo_sidebar #17211b;
            @define-color carelo_toolbar #243029;
            @define-color carelo_pane #1e2923;
            @define-color carelo_footer #1b251f;
            @define-color carelo_control #303d35;
            @define-color carelo_accent #54c98a;
            @define-color carelo_on_accent #101214;
            "#
        .to_owned(),
        ColorTheme::Aubergine => r#"
            @define-color carelo_void #181219;
            @define-color carelo_sidebar #231a24;
            @define-color carelo_toolbar #312534;
            @define-color carelo_pane #2a202c;
            @define-color carelo_footer #261d28;
            @define-color carelo_control #403244;
            @define-color carelo_accent #c082e8;
            @define-color carelo_on_accent #101214;
            "#
        .to_owned(),
    };
    apply_appearance_with_palette(
        appearance,
        omarchy_palette.as_ref().map(OmarchyPalette::is_light),
    );
    THEME_CSS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
                );
            }
            provider
        });
        provider.load_from_string(&css);
    });
}

pub(super) fn install_omarchy_theme_monitor(
    sender: &ComponentSender<AppModel>,
) -> Option<gio::FileMonitor> {
    let directory = omarchy::current_theme_directory()?;
    let file = gio::File::for_path(&directory);
    let monitor = match file
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    {
        Ok(monitor) => monitor,
        Err(error) => {
            tracing::warn!(%error, path = %directory.display(), "could not watch the Omarchy theme");
            return None;
        }
    };
    let input = sender.input_sender().clone();
    let reload_pending = Rc::new(Cell::new(false));
    monitor.connect_changed(move |_, _, _, _| {
        if reload_pending.replace(true) {
            return;
        }
        let input = input.clone();
        let reload_pending = Rc::clone(&reload_pending);
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            reload_pending.set(false);
            let _ = input.send(AppMsg::OmarchyThemeChanged);
        });
    });
    Some(monitor)
}

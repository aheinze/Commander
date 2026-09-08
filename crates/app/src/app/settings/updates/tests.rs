use super::super::tests::{snapshot, wait_until};
use super::*;

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_updates_show_release_download_errors_and_cancel_on_close() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    install_styles(AppearanceMode::Dark);
    let window = adw::ApplicationWindow::new(&relm4::main_adw_application());
    window.set_default_size(1000, 800);
    window.present();
    let (dialog, view) = utility_dialog("Settings", 780, 680, "settings-dialog");
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.add_css_class("settings-page");
    let panel = Panel::new();
    page.append(&panel.root);
    view.set_content(Some(&page));
    dialog.present(Some(&window));
    wait_until(|| dialog.is_mapped() && panel.check.is_sensitive());
    let update = Available {
        version: semver::Version::parse("1.0.0").unwrap(),
        notes: "Commander 1.0.0\n\n• Faster folder navigation\n• Improved previews\n\nA release with useful changes for everyday file work.".into(),
        notice: String::new(),
        installation: updater::Installation { preferred: Some(updater::Format::AppImage), description: "AppImage installation. Download the new AppImage and replace it after closing Commander.".into() },
        assets: vec![updater::Asset {
            name: "commander-1.0.0-1-x86_64.AppImage".into(), architecture: "x86_64".into(),
            format: updater::Format::AppImage, size: 7, sha256: "0".repeat(64), glibc_minimum: "2.35".into(),
        }],
    };
    panel.handle(Event::Checked(
        Cache::default(),
        Box::new(Ok(Outcome::Available(update.clone()))),
    ));
    assert!(panel.details.is_visible() && panel.download.is_visible());
    assert_eq!(
        panel.release.uri(),
        "https://github.com/aheinze/Commander/releases/tag/v1.0.0"
    );
    panel.set_busy(true);
    panel.progress.set_visible(true);
    panel.handle(Event::Progress(3));
    assert!((panel.progress.fraction() - 3.0 / 7.0).abs() < 0.001);
    assert!(!panel.check.is_sensitive() && !panel.skip.is_sensitive());
    panel.handle(Event::Downloaded(Err(updater::Failure {
        message: "Download verification failed. The incomplete or changed file was removed.".into(),
        retry_at: None,
    })));
    assert!(panel.check.is_sensitive());
    assert!(!panel.reveal.is_visible());
    assert!(!panel.status.text().contains("up to date"));
    panel.show_available(update.clone());
    snapshot(&dialog, "settings-update-available-dark");
    apply_appearance(AppearanceMode::Light);
    dialog.set_content_width(620);
    snapshot(&dialog, "settings-update-available-compact-light");
    panel.skip.emit_clicked();
    wait_until(|| Cache::load(&Cache::path().unwrap()).skipped.as_deref() == Some("1.0.0"));
    assert!(!panel.details.is_visible());
    assert_eq!(panel.cache.borrow().skipped.as_deref(), Some("1.0.0"));
    let mut unsigned = update;
    unsigned.assets.clear();
    unsigned.notice =
        "This release has no signed update metadata. View the release on GitHub for downloads."
            .into();
    panel.show_available(unsigned);
    assert!(!panel.download.is_visible());
    assert!(panel.release.is_visible());
    let pending = panel.cancel.borrow().clone();
    panel.close();
    let previous = panel.status.text();
    panel.handle(Event::Downloaded(Ok(PathBuf::from("/must-not-be-opened"))));
    assert_eq!(panel.status.text(), previous);
    assert!(pending.is_cancelled());
    dialog.close();
    window.close();
}

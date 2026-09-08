use super::*;
use relm4::{Component, ComponentController};

#[test]
fn everyday_actions_lead_and_specialist_tools_follow_the_target() {
    let file = (VPath::from("/tmp/document.PDF"), EntryKind::File);
    let groups = menu::sections(Some(&file), 1);
    assert_eq!(groups[0].1[0].command, CommandId::Open);
    let common: Vec<_> = groups
        .iter()
        .filter(|(more, _)| !more)
        .flat_map(|(_, rows)| rows.iter().map(|row| row.command))
        .collect();
    assert!(common.contains(&CommandId::CopyClipboard) && common.contains(&CommandId::Trash));
    assert!(
        !common.contains(&CommandId::Checksum) && !common.contains(&CommandId::DeletePermanent)
    );
    assert!(
        !common.contains(&CommandId::Paste),
        "Pasting into a file has no useful meaning"
    );
    assert!(
        groups
            .iter()
            .flat_map(|(_, rows)| rows)
            .any(|row| row.command == CommandId::PdfTools)
    );
    let fifo = (file.0.clone(), EntryKind::Fifo);
    assert!(
        !menu::sections(Some(&fifo), 1)
            .iter()
            .flat_map(|(_, rows)| rows)
            .any(|row| matches!(
                row.command,
                CommandId::PdfTools | CommandId::Checksum | CommandId::ConvertImage
            ))
    );
    let multi: Vec<_> = menu::sections(Some(&file), 3)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| row.command)
        .collect();
    assert!(multi.contains(&CommandId::BatchRename));
    assert!(!multi.contains(&CommandId::Rename) && !multi.contains(&CommandId::OpenWith));
    let background: Vec<_> = menu::sections(None, 0)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .map(|row| row.command)
        .collect();
    assert!(background.contains(&CommandId::Paste));
    assert!(!background.contains(&CommandId::Trash));
}

#[test]
#[ignore = "requires an isolated GTK session; run alone with --ignored --test-threads=1"]
fn gtk_context_menu_search_keyboard_selection_and_paste_destination() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let name = format!(
        "Design review {}.pdf",
        "with a long descriptive file name ".repeat(3)
    );
    let document = fixture.path().join(&name);
    let folder = fixture.path().join("Destination");
    std::fs::write(&document, b"Context menu paste fixture").unwrap();
    std::fs::create_dir(&folder).unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(fixture.path())),
                right: Some(VPath::from(fixture.path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(SessionState {
                sidebar_visible: false,
                preview_visible: false,
                dual_pane: false,
                window_width: 900,
                window_height: 720,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| !app.model().pane(PaneId::Left).loading);
    let target = (VPath::from(document.as_path()), EntryKind::File);
    let popover = open(&app, Some(&target));
    let search = find::<gtk::SearchEntry>(popover.upcast_ref()).unwrap();
    wait_until(|| focused_label(&app).as_deref() == Some("Open"));
    assert!(visible_button(&popover, "Verify checksum…").is_none());
    assert!(visible_button(&popover, "Delete permanently…").is_none());
    let old_cursor = app.model().pane(PaneId::Left).cursor_row;
    key(&app, &popover, gdk::Key::Down);
    assert_eq!(focused_label(&app).as_deref(), Some("Quick Look"));
    assert_eq!(app.model().pane(PaneId::Left).cursor_row, old_cursor);
    app.widget().set_focus_visible(true);
    snapshot(&app, &popover, "context-common-dark");
    apply_appearance(AppearanceMode::Light);
    snapshot(&app, &popover, "context-common-light");
    apply_appearance(AppearanceMode::Dark);
    visible_button(&popover, "More actions")
        .unwrap()
        .emit_clicked();
    assert!(visible_button(&popover, "Verify checksum…").is_some());
    assert!(visible_button(&popover, "Back").is_some());
    snapshot(&app, &popover, "context-more");
    key(&app, &popover, gdk::Key::Left);
    assert!(visible_button(&popover, "More actions").is_some());
    key(&app, &popover, gdk::Key::c);
    assert_eq!(search.text(), "c", "Typing in the menu starts searching");
    assert!(app.model().pane(PaneId::Left).filter_query.is_empty());
    search.set_text("CHECKSUM");
    assert!(visible_button(&popover, "Verify checksum…").is_some());
    assert!(visible_button(&popover, "Open").is_none());
    snapshot(&app, &popover, "context-search");
    search.set_text("no action has this name");
    key(&app, &popover, gdk::Key::Return);
    assert!(
        popover.is_visible(),
        "Enter on an empty result must do nothing"
    );
    search.set_text("blue");
    assert!(visible_button(&popover, "Set blue tag").is_some());
    assert!(visible_button(&popover, "Set red tag").is_none());
    search.set_text("copy path");
    key(&app, &popover, gdk::Key::Return);
    wait_until(|| popover.parent().is_none());
    let copied = glib::MainContext::default()
        .block_on(app.widget().clipboard().read_text_future())
        .unwrap()
        .unwrap();
    assert_eq!(copied.as_str(), document.to_str().unwrap());
    let weak = popover.downgrade();
    drop(search);
    drop(popover);
    wait_until(|| weak.upgrade().is_none());

    let folder_target = (VPath::from(folder.as_path()), EntryKind::Directory);
    let popover = open(&app, Some(&folder_target));
    let paste = visible_button(&popover, "Paste into folder").unwrap();
    assert!(
        !paste.is_sensitive(),
        "Plain text cannot be pasted as files"
    );
    let provider =
        super::super::clipboard::provider(std::slice::from_ref(&target.0), false).unwrap();
    app.widget()
        .clipboard()
        .set_content(Some(&provider))
        .unwrap();
    wait_until(|| paste.is_sensitive());
    snapshot(&app, &popover, "context-folder");
    paste.emit_clicked();
    wait_until(|| folder.join(&name).is_file() && app.model().active_operations == 0);
    assert_eq!(
        std::fs::read(folder.join(&name)).unwrap(),
        b"Context menu paste fixture"
    );

    app.emit(AppMsg::SelectAllActive);
    wait_until(|| app.model().pane(PaneId::Left).selection.len() == 2);
    let popover = open(&app, Some(&target));
    assert!(visible_button(&popover, "Rename selected items…").is_some());
    assert!(visible_button(&popover, "Rename…").is_none());
    snapshot(&app, &popover, "context-multiple");
    key(&app, &popover, gdk::Key::Escape);
    wait_until(|| popover.parent().is_none());
    assert_eq!(
        app.model().pane(PaneId::Left).selection.len(),
        2,
        "Escape dismisses the menu without clearing selection"
    );
    let popover = open(&app, None);
    assert!(visible_button(&popover, "New folder…").is_some());
    assert!(visible_button(&popover, "Move to Trash").is_none());
    snapshot(&app, &popover, "context-background");
    popover.popdown();

    app.widget().set_default_size(720, 480);
    wait_until(|| app.widget().height() == 480);
    let popover = open(&app, Some(&target));
    visible_button(&popover, "More actions")
        .unwrap()
        .emit_clicked();
    snapshot(&app, &popover, "context-compact");
    key(&app, &popover, gdk::Key::End);
    assert_eq!(focused_label(&app).as_deref(), Some("Clear tag"));
    let scroll = find::<gtk::ScrolledWindow>(popover.upcast_ref()).unwrap();
    assert!(
        scroll.vadjustment().value() > 0.0,
        "Keyboard focus must scroll into view"
    );
    popover.popdown();
    app.widget().close();
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_tab_folder_menu_targets_inactive_tabs_and_preserves_selection() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("Inactive tab folder");
    let other = fixture.path().join("Other pane");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let document = fixture.path().join("keep.txt");
    std::fs::write(&document, "keep this file").unwrap();
    std::fs::write(other.join("also-keep.txt"), "keep other pane").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions::default(),
            session_worker,
            session: Some(SessionState {
                left: PaneSession {
                    tabs: vec![
                        fixture.path().display().to_string(),
                        folder.display().to_string(),
                    ],
                    ..PaneSession::default()
                },
                right: PaneSession {
                    tabs: vec![other.display().to_string()],
                    ..PaneSession::default()
                },
                sidebar_visible: false,
                preview_visible: false,
                window_width: 1100,
                window_height: 800,
                ..SessionState::default()
            }),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| !app.model().panes.iter().any(|pane| pane.loading));
    app.emit(AppMsg::SelectAllActive);
    wait_until(|| app.model().pane(PaneId::Left).selection.len() == 3);
    let selection = app.model().pane(PaneId::Left).selection.clone();
    let sources = app.model().operation_sources(PaneId::Left);

    let popover = open_tab(&app, PaneId::Left, 1, false);
    assert!(visible_button(&popover, "Paste into folder").is_some());
    assert!(visible_button(&popover, "Rename…").is_some());
    assert!(visible_button(&popover, "Rename selected items…").is_none());
    assert_eq!(app.model().pane(PaneId::Left).active_tab, 0);
    snapshot(&app, &popover, "tab-folder-dark");
    apply_appearance(AppearanceMode::Light);
    snapshot(&app, &popover, "tab-folder-light");
    apply_appearance(AppearanceMode::Dark);
    find::<gtk::SearchEntry>(popover.upcast_ref())
        .unwrap()
        .set_text("copy path");
    visible_button(&popover, "Copy path")
        .unwrap()
        .emit_clicked();
    wait_until(|| popover.parent().is_none());
    wait_until(|| app.model().folder_action_target.is_none());
    // Drain the action forwarder before checking the clipboard.
    wait_until(|| {
        glib::MainContext::default()
            .block_on(app.widget().clipboard().read_text_future())
            .ok()
            .flatten()
            .as_deref()
            == folder.to_str()
    });
    assert_eq!(app.model().operation_sources(PaneId::Left), sources);
    assert_eq!(app.model().pane(PaneId::Left).selection, selection);

    let popover = open_tab(&app, PaneId::Left, 1, true);
    find::<gtk::SearchEntry>(popover.upcast_ref())
        .unwrap()
        .set_text("blue");
    visible_button(&popover, "Set blue tag")
        .unwrap()
        .emit_clicked();
    wait_until(|| {
        app.model()
            .tags
            .get(&folder.display().to_string())
            .map(String::as_str)
            == Some("blue")
    });
    assert!(
        !app.model()
            .tags
            .contains_key(&document.display().to_string())
    );
    assert!(app.model().folder_action_target.is_none());

    let provider =
        super::super::clipboard::provider(&[VPath::from(document.as_path())], false).unwrap();
    app.widget()
        .clipboard()
        .set_content(Some(&provider))
        .unwrap();
    let popover = open_tab(&app, PaneId::Left, 1, false);
    let paste = visible_button(&popover, "Paste into folder").unwrap();
    wait_until(|| paste.is_sensitive());
    paste.emit_clicked();
    wait_until(|| folder.join("keep.txt").is_file() && app.model().active_operations == 0);
    assert!(!other.join("keep.txt").exists());

    let popover = open_tab(&app, PaneId::Left, 1, false);
    visible_button(&popover, "Rename…").unwrap().emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    assert_eq!(
        find::<gtk::Entry>(dialog.upcast_ref()).unwrap().text(),
        "Inactive tab folder"
    );
    dialog.close();
    wait_until(|| app.widget().visible_dialog().is_none());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode() & 0o7777;
        let popover = open_tab(&app, PaneId::Left, 1, false);
        find::<gtk::SearchEntry>(popover.upcast_ref())
            .unwrap()
            .set_text("permissions");
        visible_button(&popover, "Permissions…")
            .unwrap()
            .emit_clicked();
        wait_until(|| app.widget().visible_dialog().is_some());
        let dialog = app.widget().visible_dialog().unwrap();
        assert_eq!(
            find::<gtk::Entry>(dialog.upcast_ref()).unwrap().text(),
            format!("{mode:04o}")
        );
        dialog.close();
        wait_until(|| app.widget().visible_dialog().is_none());
    }

    let popover = open_tab(&app, PaneId::Left, 1, false);
    find::<gtk::SearchEntry>(popover.upcast_ref())
        .unwrap()
        .set_text("quick look");
    visible_button(&popover, "Quick Look")
        .unwrap()
        .emit_clicked();
    wait_until(|| {
        app.model().quick_look_open
            && app.model().preview_state.path.as_ref() == Some(&VPath::from(folder.as_path()))
    });
    app.emit(AppMsg::ToggleQuickLook);
    wait_until(|| !app.model().quick_look_open && app.widget().visible_dialog().is_none());

    let popover = open_tab(&app, PaneId::Left, 1, false);
    find::<gtk::SearchEntry>(popover.upcast_ref())
        .unwrap()
        .set_text("create archive");
    visible_button(&popover, "Create archive…")
        .unwrap()
        .emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    find::<gtk::Entry>(dialog.upcast_ref())
        .unwrap()
        .set_text("Tab archive");
    app.emit(AppMsg::ActivatePane(PaneId::Right));
    wait_until(|| app.model().active_pane == PaneId::Right);
    dialog_response(&dialog, "Create").emit_clicked();
    let archive_path = fixture.path().join("Tab archive.zip");
    wait_until(|| archive_path.is_file() && app.model().active_operations == 0);
    let mut archive = zip::ZipArchive::new(std::fs::File::open(archive_path).unwrap()).unwrap();
    assert!(archive.by_name("Inactive tab folder/keep.txt").is_ok());
    assert!(archive.by_name("keep.txt").is_err());

    let popover = open_tab(&app, PaneId::Right, 0, false);
    find::<gtk::SearchEntry>(popover.upcast_ref())
        .unwrap()
        .set_text("copy path");
    visible_button(&popover, "Copy path")
        .unwrap()
        .emit_clicked();
    wait_until(|| {
        glib::MainContext::default()
            .block_on(app.widget().clipboard().read_text_future())
            .ok()
            .flatten()
            .as_deref()
            == other.to_str()
    });

    let popover = open_tab(&app, PaneId::Left, 1, false);
    visible_button(&popover, "Open in new tab")
        .unwrap()
        .emit_clicked();
    wait_until(|| {
        app.model().pane(PaneId::Left).tabs.len() == 3 && !app.model().pane(PaneId::Left).loading
    });
    assert_eq!(
        app.model().pane(PaneId::Left).active().path,
        VPath::from(folder.as_path())
    );
    app.emit(AppMsg::CloseTabAt(PaneId::Left, 2));
    app.emit(AppMsg::SelectTab(PaneId::Left, 0));
    wait_until(|| {
        app.model().pane(PaneId::Left).active_tab == 0 && !app.model().pane(PaneId::Left).loading
    });

    let popover = open_tab(&app, PaneId::Left, 1, false);
    find::<gtk::SearchEntry>(popover.upcast_ref())
        .unwrap()
        .set_text("delete permanently");
    visible_button(&popover, "Delete permanently…")
        .unwrap()
        .emit_clicked();
    wait_until(|| app.widget().visible_dialog().is_some());
    let dialog = app.widget().visible_dialog().unwrap();
    app.emit(AppMsg::ActivatePane(PaneId::Right));
    wait_until(|| app.model().active_pane == PaneId::Right);
    dialog_response(&dialog, "Delete").emit_clicked();
    wait_until(|| !folder.exists() && app.model().active_operations == 0);
    assert!(document.is_file());
    assert!(other.join("also-keep.txt").is_file());
    assert!(app.model().folder_action_target.is_none());
    app.widget().close();
}

fn dialog_response(dialog: &adw::Dialog, label: &str) -> gtk::Button {
    fn visit(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && button.label().as_deref() == Some(label)
        {
            return Some(button.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(button) = visit(&widget, label) {
                return Some(button);
            }
        }
        None
    }
    visit(dialog.upcast_ref(), label).unwrap()
}

fn open_tab(
    app: &relm4::Controller<AppModel>,
    pane: PaneId,
    tab: usize,
    keyboard: bool,
) -> gtk::Popover {
    let parent = app.widgets().panes[pane.index()].root.clone();
    let mut pill = app.widgets().panes[pane.index()]
        .tab_bar
        .first_child()
        .unwrap();
    for _ in 0..tab {
        pill = pill.next_sibling().unwrap();
    }
    wait_until(|| pill.width() > 0);
    let controllers = pill.observe_controllers();
    for index in 0..controllers.n_items() {
        let controller = controllers.item(index).unwrap();
        if keyboard {
            if let Some(keys) = controller.downcast_ref::<gtk::EventControllerKey>() {
                assert!(keys.emit_by_name::<bool>(
                    "key-pressed",
                    &[&gdk::Key::F10, &0_u32, &gdk::ModifierType::SHIFT_MASK]
                ));
            }
        } else if let Some(gesture) = controller.downcast_ref::<gtk::GestureClick>()
            && gesture.button() == 3
        {
            gesture.emit_by_name::<()>("pressed", &[&1_i32, &12.0_f64, &12.0_f64]);
        }
    }
    let popover = parent.last_child().and_downcast::<gtk::Popover>().unwrap();
    assert!(popover.has_css_class("file-context-menu"));
    wait_until(|| popover.is_mapped());
    popover
}

fn open(app: &relm4::Controller<AppModel>, target: Option<&(VPath, EntryKind)>) -> gtk::Popover {
    let view = app.widgets().panes[0].column_view.clone();
    wait_until(|| view.height() > 0);
    let mut y = None;
    wait_until(|| {
        y = (10..view.height())
            .find(|y| context_target_at(view.upcast_ref(), 60.0, f64::from(*y)).as_ref() == target);
        y.is_some()
    });
    let y = y.unwrap();
    let controllers = view.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(gesture) = controllers.item(index).and_downcast::<gtk::GestureClick>()
            && gesture.button() == 3
        {
            gesture.emit_by_name::<()>("pressed", &[&1_i32, &60.0_f64, &f64::from(y)]);
        }
    }
    let mut child = view.first_child();
    let popover = loop {
        let widget = child.expect("opened context menu");
        child = widget.next_sibling();
        if let Ok(popover) = widget.downcast::<gtk::Popover>() {
            break popover;
        }
    };
    wait_until(|| popover.is_mapped());
    popover
}

fn find<T: IsA<gtk::Widget> + glib::object::IsClass>(widget: &gtk::Widget) -> Option<T> {
    if let Ok(found) = widget.clone().downcast::<T>() {
        return Some(found);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(found) = find(&widget) {
            return Some(found);
        }
    }
    None
}

fn button_label(button: &gtk::Button) -> Option<String> {
    find::<gtk::Label>(button.upcast_ref())
        .map(|label| label.text().to_string())
        .or_else(|| button.tooltip_text().map(|text| text.to_string()))
}

fn visible_button(popover: &gtk::Popover, label: &str) -> Option<gtk::Button> {
    fn visit(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if !widget.is_visible() {
            return None;
        }
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && button_label(button).as_deref() == Some(label)
        {
            return Some(button.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Some(found) = visit(&widget, label) {
                return Some(found);
            }
        }
        None
    }
    visit(popover.upcast_ref(), label)
}

fn focused_label(app: &relm4::Controller<AppModel>) -> Option<String> {
    gtk::prelude::GtkWindowExt::focus(app.widget())
        .and_downcast::<gtk::Button>()
        .and_then(|button| button_label(&button))
}

fn key(app: &relm4::Controller<AppModel>, popover: &gtk::Popover, key: gdk::Key) {
    // Exercise window capture first: it must leave menu keys to the popover.
    for widget in [
        app.widget().upcast_ref::<gtk::Widget>(),
        popover.upcast_ref(),
    ] {
        let controllers = widget.observe_controllers();
        for index in 0..controllers.n_items() {
            if let Some(controller) = controllers
                .item(index)
                .and_downcast::<gtk::EventControllerKey>()
            {
                let stopped = controller.emit_by_name::<bool>(
                    "key-pressed",
                    &[&key, &0_u32, &gdk::ModifierType::empty()],
                );
                if stopped {
                    assert_eq!(
                        widget,
                        popover.upcast_ref::<gtk::Widget>(),
                        "Window shortcut consumed a context-menu key"
                    );
                    return;
                }
            }
        }
    }
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let context = glib::MainContext::default();
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "context-menu condition timed out"
        );
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(app: &relm4::Controller<AppModel>, popover: &gtk::Popover, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(160);
    let context = glib::MainContext::default();
    while Instant::now() < deadline {
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        popover.width() <= 410,
        "Menu must not expand around long names or shortcuts: {}px",
        popover.width()
    );
    assert!(
        popover.height() <= app.widget().height(),
        "Menu must fit the window height"
    );
    let directory = std::env::var_os("COMMANDER_CONTEXT_SNAPSHOT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("COMMANDER_TEST_ARTIFACTS")
                .map(|path| std::path::PathBuf::from(path).join("snapshots"))
        });
    let Some(directory) = directory else {
        return;
    };
    let child = popover.first_child().unwrap();
    let snapshot = gtk::Snapshot::new();
    popover.snapshot_child(&child, &snapshot);
    let node = snapshot.to_node().unwrap();
    popover
        .renderer()
        .unwrap()
        .render_texture(&node, None)
        .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
        .unwrap();
}

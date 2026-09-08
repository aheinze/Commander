//! Stable terminal tabs with a compact header and explicit overflow controls.
use super::*;

struct TerminalTab {
    root: gtk::Box,
    select: gtk::Button,
    label: gtk::Label,
    status: gtk::Label,
    close: gtk::Button,
}

pub(super) struct TerminalTabs {
    pub(super) root: gtk::Box,
    tabs: gtk::Box,
    scroll: gtk::ScrolledWindow,
    #[cfg(test)]
    previous: gtk::Button,
    #[cfg(test)]
    next: gtk::Button,
    #[cfg(test)]
    new: gtk::Button,
    close_all: gtk::Button,
    rows: RefCell<BTreeMap<u64, TerminalTab>>,
    active: Rc<RefCell<Option<glib::WeakRef<gtk::Box>>>>,
    rendered_active: Cell<Option<u64>>,
}

fn reveal_tab(tabs: &gtk::Box, scroll: &gtk::ScrolledWindow, tab: &gtk::Box) {
    if let Some(bounds) = tab.compute_bounds(tabs) {
        let start = f64::from(bounds.x());
        scroll
            .hadjustment()
            .clamp_page(start, start + f64::from(bounds.width()));
    }
}

impl TerminalTabs {
    pub(super) fn new(input: &relm4::Sender<AppMsg>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("terminal-header");
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabs.add_css_class("terminal-tabs");
        tabs.set_halign(gtk::Align::Start);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .child(&tabs)
            .build();
        scroll.add_css_class("terminal-tabs-scroll");
        let previous = icon_button(
            "commander-chevron-left-symbolic",
            "Scroll terminal tabs left",
        );
        let next = icon_button(
            "commander-chevron-right-symbolic",
            "Scroll terminal tabs right",
        );
        for (button, direction) in [(&previous, -1.0), (&next, 1.0)] {
            button.add_css_class("terminal-action");
            button.add_css_class("terminal-tabs-arrow");
            button.set_valign(gtk::Align::Center);
            button.set_visible(false);
            let adjustment = scroll.hadjustment();
            button.connect_clicked(move |_| {
                adjustment.set_value(adjustment.value() + direction * adjustment.page_size() * 0.8);
            });
        }
        root.append(&previous);
        root.append(&scroll);
        root.append(&next);
        let active: Rc<RefCell<Option<glib::WeakRef<gtk::Box>>>> = Rc::new(RefCell::new(None));
        {
            let previous = previous.downgrade();
            let next = next.downgrade();
            scroll.hadjustment().connect_changed(move |adjustment| {
                let previous = previous.clone();
                let next = next.clone();
                let adjustment = adjustment.clone();
                // Adjustment bounds change during allocation. Defer visibility
                // changes so GTK can finish measuring the current header first.
                glib::idle_add_local_once(move || {
                    if let (Some(previous), Some(next)) = (previous.upgrade(), next.upgrade()) {
                        let arrow_width: i32 = [&previous, &next]
                            .into_iter()
                            .filter(|button| button.is_visible())
                            .map(|button| button.width())
                            .sum();
                        let available = adjustment.page_size() + f64::from(arrow_width);
                        let overflow = adjustment.upper() - adjustment.lower() > available + 1.0;
                        previous.set_visible(overflow);
                        next.set_visible(overflow);
                        previous.set_sensitive(adjustment.value() > adjustment.lower() + 1.0);
                        next.set_sensitive(
                            adjustment.value() + adjustment.page_size() < adjustment.upper() - 1.0,
                        );
                    }
                });
            });
        }
        {
            let previous = previous.downgrade();
            let next = next.downgrade();
            scroll
                .hadjustment()
                .connect_value_changed(move |adjustment| {
                    if let (Some(previous), Some(next)) = (previous.upgrade(), next.upgrade()) {
                        previous.set_sensitive(adjustment.value() > adjustment.lower() + 1.0);
                        next.set_sensitive(
                            adjustment.value() + adjustment.page_size() < adjustment.upper() - 1.0,
                        );
                    }
                });
        }
        {
            let tabs = tabs.downgrade();
            let scroll_weak = scroll.downgrade();
            let active = Rc::clone(&active);
            scroll.hadjustment().connect_changed(move |_| {
                if let (Some(tabs), Some(scroll), Some(tab)) = (
                    tabs.upgrade(),
                    scroll_weak.upgrade(),
                    active.borrow().as_ref().and_then(glib::WeakRef::upgrade),
                ) {
                    reveal_tab(&tabs, &scroll, &tab);
                }
            });
        }
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        {
            let adjustment = scroll.hadjustment();
            wheel.connect_scroll(move |_, dx, dy| {
                if adjustment.upper() <= adjustment.page_size() {
                    return glib::Propagation::Proceed;
                }
                let delta = if dx.abs() > dy.abs() { dx } else { dy };
                adjustment.set_value(adjustment.value() + delta * 40.0);
                glib::Propagation::Stop
            });
        }
        scroll.add_controller(wheel);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        actions.add_css_class("terminal-actions");
        let new = icon_button(
            "commander-plus-symbolic",
            "New terminal in the active folder",
        );
        let hide = icon_button("commander-chevron-down-symbolic", "Hide terminal · Ctrl+`");
        let menu = gtk::MenuButton::builder()
            .icon_name("commander-ellipsis-symbolic")
            .tooltip_text("Terminal options")
            .valign(gtk::Align::Center)
            .build();
        menu.add_css_class("terminal-menu");
        let (popover, menu_actions) = context_menu::context_action_menu();
        let close_all =
            context_menu_item_button("Close all terminals", "commander-x-symbolic", None);
        menu_actions.append(&close_all);
        menu.set_popover(Some(&popover));
        for (button, create) in [(&new, true), (&hide, false)] {
            button.add_css_class("flat");
            button.add_css_class("terminal-action");
            button.set_valign(gtk::Align::Center);
            let input = input.clone();
            button.connect_clicked(move |_| {
                let _ = input.send(if create {
                    AppMsg::NewTerminal
                } else {
                    AppMsg::ToggleTerminal
                });
            });
        }
        {
            let input = input.clone();
            let popover = popover.downgrade();
            close_all.connect_clicked(move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                let _ = input.send(AppMsg::CloseAllTerminals);
            });
        }
        actions.append(&new);
        actions.append(&menu);
        actions.append(&hide);
        root.append(&actions);
        Self {
            root,
            tabs,
            scroll,
            #[cfg(test)]
            previous,
            #[cfg(test)]
            next,
            #[cfg(test)]
            new,
            close_all,
            rows: RefCell::new(BTreeMap::new()),
            active,
            rendered_active: Cell::new(None),
        }
    }

    pub(super) fn render(
        &self,
        states: &[TerminalTabState],
        active: Option<u64>,
        input: &relm4::Sender<AppMsg>,
        surface: &gtk::DrawingArea,
    ) {
        self.close_all.set_sensitive(!states.is_empty());
        let mut rows = self.rows.borrow_mut();
        rows.retain(|id, row| {
            let keep = states.iter().any(|tab| tab.id == *id);
            if !keep {
                self.tabs.remove(&row.root);
            }
            keep
        });
        for state in states {
            let row = rows.entry(state.id).or_insert_with(|| {
                let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                root.add_css_class("terminal-tab");
                let select = gtk::Button::new();
                select.add_css_class("flat");
                select.add_css_class("terminal-tab-main");
                select.set_hexpand(true);
                let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let icon = gtk::Image::from_icon_name("commander-terminal-symbolic");
                icon.set_pixel_size(14);
                let label = gtk::Label::builder()
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .max_width_chars(16)
                    .single_line_mode(true)
                    .build();
                let status = gtk::Label::new(None);
                status.add_css_class("terminal-tab-status");
                content.append(&icon);
                content.append(&label);
                content.append(&status);
                select.set_child(Some(&content));
                let close = icon_button("commander-x-symbolic", "Close terminal");
                close.add_css_class("flat");
                close.add_css_class("terminal-tab-close");
                close.set_valign(gtk::Align::Center);
                for (button, closing) in [(&select, false), (&close, true)] {
                    let input = input.clone();
                    let surface = surface.downgrade();
                    let id = state.id;
                    button.connect_clicked(move |_| {
                        if let Some(surface) = surface.upgrade() {
                            surface.grab_focus();
                        }
                        let _ = input.send(if closing {
                            AppMsg::CloseTerminal(id)
                        } else {
                            AppMsg::SelectTerminal(id)
                        });
                    });
                }
                let focus = gtk::EventControllerFocus::new();
                {
                    let tabs = self.tabs.downgrade();
                    let scroll = self.scroll.downgrade();
                    let root = root.downgrade();
                    focus.connect_enter(move |_| {
                        if let (Some(tabs), Some(scroll), Some(root)) =
                            (tabs.upgrade(), scroll.upgrade(), root.upgrade())
                        {
                            reveal_tab(&tabs, &scroll, &root);
                        }
                    });
                }
                root.add_controller(focus);
                root.append(&select);
                root.append(&close);
                self.tabs.append(&root);
                TerminalTab {
                    root,
                    select,
                    label,
                    status,
                    close,
                }
            });
            let title = if states.iter().filter(|tab| tab.title == state.title).count() > 1 {
                format!("{} · {}", state.title, state.id)
            } else {
                state.title.clone()
            };
            let status = if state.starting {
                "Starting…"
            } else if state.exited {
                "Exited"
            } else {
                ""
            };
            row.label.set_label(&title);
            row.status.set_label(status);
            row.status.set_visible(!status.is_empty());
            row.select.set_tooltip_text(Some(&format!(
                "{title}\nStarted in {}{}",
                state.cwd,
                if status.is_empty() {
                    String::new()
                } else {
                    format!("\n{status}")
                }
            )));
            row.select
                .update_property(&[gtk::accessible::Property::Label(&format!(
                    "{title} terminal {status}"
                ))]);
            row.select.update_state(&[gtk::accessible::State::Pressed(
                if active == Some(state.id) {
                    gtk::AccessibleTristate::True
                } else {
                    gtk::AccessibleTristate::False
                },
            )]);
            row.close
                .set_tooltip_text(Some(&format!("Close {title} terminal")));
            if active == Some(state.id) {
                row.root.add_css_class("terminal-tab-active");
            } else {
                row.root.remove_css_class("terminal-tab-active");
            }
        }
        *self.active.borrow_mut() = active
            .and_then(|id| rows.get(&id))
            .map(|row| row.root.downgrade());
        if self.rendered_active.replace(active) != active {
            // The newly appended tab needs an allocation before it can be revealed.
            let active = Rc::clone(&self.active);
            let scroll = self.scroll.downgrade();
            let first_frame = Cell::new(true);
            self.tabs.add_tick_callback(move |tabs, _| {
                if first_frame.replace(false) {
                    return glib::ControlFlow::Continue;
                }
                if let (Some(scroll), Some(tab)) = (
                    scroll.upgrade(),
                    active.borrow().as_ref().and_then(glib::WeakRef::upgrade),
                ) {
                    reveal_tab(tabs, &scroll, &tab);
                }
                glib::ControlFlow::Break
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relm4::{Component, ComponentController};

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "terminal UI timed out");
            drain_frames();
        }
    }

    fn drain_frames() {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_millis(120);
        while Instant::now() < deadline {
            while context.pending() {
                context.iteration(false);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn assert_visible(strip: &TerminalTabs, id: u64) {
        let bounds = strip.rows.borrow()[&id]
            .root
            .compute_bounds(&strip.tabs)
            .unwrap();
        let adjustment = strip.scroll.hadjustment();
        assert!(
            f64::from(bounds.x()) >= adjustment.value() - 1.0,
            "tab start is visible"
        );
        assert!(
            f64::from(bounds.x() + bounds.width())
                <= adjustment.value() + adjustment.page_size() + 1.0,
            "tab end is visible"
        );
    }

    fn snapshot(window: &adw::ApplicationWindow, name: &str) {
        let Some(directory) = std::env::var_os("COMMANDER_TERMINAL_SNAPSHOT_DIR") else {
            return;
        };
        let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
        let snapshot = gtk::Snapshot::new();
        window.snapshot_child(&child, &snapshot);
        window
            .renderer()
            .unwrap()
            .render_texture(snapshot.to_node().unwrap(), None)
            .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
            .unwrap();
    }

    fn snapshot_menu(menu: &gtk::Popover, name: &str) {
        drain_frames();
        let Some(directory) = std::env::var_os("COMMANDER_TERMINAL_SNAPSHOT_DIR") else {
            return;
        };
        let child = menu.first_child().unwrap();
        let snapshot = gtk::Snapshot::new();
        menu.snapshot_child(&child, &snapshot);
        menu.renderer()
            .unwrap()
            .render_texture(snapshot.to_node().unwrap(), None)
            .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
            .unwrap();
    }

    #[test]
    #[ignore = "requires an isolated GTK session and SHELL=/bin/sh; run alone with --ignored --test-threads=1"]
    fn gtk_terminal_tabs_focus_overflow_and_lifecycle() {
        assert_eq!(std::env::var("SHELL").as_deref(), Ok("/bin/sh"));
        adw::init().unwrap();
        relm4::main_adw_application()
            .register(gio::Cancellable::NONE)
            .unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("Project with a long folder name");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("README.md"), "Terminal fixture\n").unwrap();
        let (session_worker, startup) = SessionWorker::start().unwrap();
        let app = AppModel::builder()
            .launch(AppInit {
                options: AppOptions {
                    left: Some(VPath::from(project.as_path())),
                    right: Some(VPath::from(project.as_path())),
                    ..AppOptions::default()
                },
                session_worker,
                session: Some(SessionState {
                    dual_pane: false,
                    sidebar_visible: false,
                    preview_visible: false,
                    appearance: AppearanceMode::Dark,
                    // Keep the eight-tab overflow fixture independent of the
                    // default application window size and compositor display.
                    window_width: 900,
                    window_height: 600,
                    ..SessionState::default()
                }),
                keymap_overrides: startup.keymap_overrides,
                history: startup.history,
                history_warning: startup.history_warning,
                started: Instant::now(),
            })
            .detach();
        app.widget().present();
        app.widgets().terminal.tabs.new.emit_clicked();
        wait_until(|| {
            app.model().terminal_tabs.len() == 1 && !app.model().terminal_tabs[0].starting
        });
        // Headless compositors may have no active seat. Assert the window's
        // keyboard focus target independently of desktop activation.
        wait_until(|| app.widgets().terminal.surface.is_focus());
        app.emit(AppMsg::TerminalInput(
            b"printf 'terminal tab one\\n'\n".to_vec(),
        ));
        wait_until(|| {
            app.model().terminal_tabs[0]
                .screen
                .borrow()
                .parser
                .screen()
                .contents()
                .contains("terminal tab one")
        });
        let first = app.model().terminal_tabs[0].id;
        let options = app
            .widgets()
            .terminal
            .tabs
            .close_all
            .ancestor(gtk::Popover::static_type())
            .and_downcast::<gtk::Popover>()
            .unwrap();
        let terminal_menu = app
            .widgets()
            .terminal
            .surface
            .first_child()
            .and_downcast::<gtk::Popover>()
            .unwrap();
        for (appearance, name) in [
            (AppearanceMode::Dark, "dark"),
            (AppearanceMode::Light, "light"),
        ] {
            apply_appearance(appearance);
            options.popup();
            wait_until(|| options.is_mapped());
            snapshot_menu(&options, &format!("terminal-options-{name}"));
            options.popdown();
            terminal_menu.popup();
            wait_until(|| terminal_menu.is_mapped());
            snapshot_menu(&terminal_menu, &format!("terminal-context-{name}"));
            terminal_menu.popdown();
        }
        apply_appearance(AppearanceMode::Dark);
        app.widgets().terminal.surface.grab_focus();
        drain_frames();
        let first_widget = app.widgets().terminal.tabs.rows.borrow()[&first]
            .root
            .clone();
        for count in 2..=8 {
            app.widgets().terminal.tabs.new.emit_clicked();
            wait_until(|| {
                app.model().terminal_tabs.len() == count
                    && app.model().terminal_tabs.iter().all(|tab| !tab.starting)
            });
        }
        let last = app.model().active_terminal.unwrap();
        drain_frames();
        {
            let widgets = app.widgets();
            let strip = &widgets.terminal.tabs;
            assert_eq!(strip.rows.borrow()[&first].root, first_widget);
            assert!(
                strip.rows.borrow()[&first]
                    .label
                    .text()
                    .ends_with(&format!("· {first}"))
            );
            assert!(strip.previous.is_visible() && strip.next.is_visible());
            assert!(strip.root.measure(gtk::Orientation::Horizontal, -1).0 <= 320);
            assert!(strip.root.height() <= 40, "header stays on one compact row");
            assert_visible(strip, last);
            let value = strip.scroll.hadjustment().value();
            strip.previous.emit_clicked();
            assert!(strip.scroll.hadjustment().value() < value);
        }
        drain_frames();
        // Switching and reselecting the active tab both return typing to its shell.
        app.widgets().terminal.tabs.rows.borrow()[&first]
            .select
            .emit_clicked();
        wait_until(|| app.model().active_terminal == Some(first));
        drain_frames();
        assert_visible(&app.widgets().terminal.tabs, first);
        assert!(app.widgets().terminal.surface.is_focus());
        let select = app.widgets().terminal.tabs.rows.borrow()[&first]
            .select
            .clone();
        select.grab_focus();
        select.emit_clicked();
        drain_frames();
        assert!(app.widgets().terminal.surface.is_focus());

        // A background exit updates its badge without replacing widgets or stealing focus.
        let close = app.widgets().terminal.tabs.rows.borrow()[&first]
            .close
            .clone();
        close.grab_focus();
        app.model()
            .terminal_tabs
            .last()
            .unwrap()
            .session
            .as_ref()
            .unwrap()
            .send(b"exit\n");
        wait_until(|| app.model().terminal_tabs.last().unwrap().exited);
        assert_eq!(
            app.widgets().terminal.tabs.rows.borrow()[&first].root,
            first_widget
        );
        assert_eq!(
            app.widgets().terminal.tabs.rows.borrow()[&last]
                .status
                .text(),
            "Exited"
        );
        assert!(close.is_focus());

        app.widgets().terminal.tabs.rows.borrow()[&last]
            .select
            .emit_clicked();
        wait_until(|| app.model().active_terminal == Some(last));
        drain_frames();
        assert_visible(&app.widgets().terminal.tabs, last);
        snapshot(app.widget(), "terminal-narrow-dark");
        app.widget().fullscreen();
        drain_frames();
        assert_visible(&app.widgets().terminal.tabs, last);
        if app.widgets().terminal.tabs.root.width() >= 1_300 {
            assert!(!app.widgets().terminal.tabs.previous.is_visible());
            assert!(!app.widgets().terminal.tabs.next.is_visible());
        }
        snapshot(app.widget(), "terminal-dark");
        apply_appearance(AppearanceMode::Light);
        drain_frames();
        snapshot(app.widget(), "terminal-light");

        // Closing a background tab leaves the active one alone; closing the active
        // tab selects its neighbour and brings it fully into view.
        close.emit_clicked();
        wait_until(|| app.model().terminal_tabs.len() == 7);
        assert_eq!(app.model().active_terminal, Some(last));
        app.widgets().terminal.tabs.rows.borrow()[&last]
            .close
            .emit_clicked();
        wait_until(|| app.model().terminal_tabs.len() == 6);
        let replacement = app.model().active_terminal.unwrap();
        assert_eq!(replacement, app.model().terminal_tabs.last().unwrap().id);
        drain_frames();
        assert_visible(&app.widgets().terminal.tabs, replacement);
        app.emit(AppMsg::ToggleTerminal);
        wait_until(|| !app.model().terminal_visible);
        assert_eq!(app.model().terminal_tabs.len(), 6);
        app.emit(AppMsg::ToggleTerminal);
        wait_until(|| app.model().terminal_visible);
        assert_eq!(app.model().active_terminal, Some(replacement));
        app.widgets().terminal.tabs.close_all.emit_clicked();
        wait_until(|| app.model().terminal_tabs.is_empty());
        assert_eq!(app.model().active_terminal, None);
        assert!(!app.widgets().terminal.tabs.close_all.is_sensitive());
        app.widgets().terminal.tabs.new.emit_clicked();
        wait_until(|| {
            app.model().terminal_tabs.len() == 1 && !app.model().terminal_tabs[0].starting
        });
        drain_frames();
        assert!(!app.widgets().terminal.tabs.previous.is_visible());
        assert!(!app.widgets().terminal.tabs.next.is_visible());
        app.emit(AppMsg::CloseAllTerminals);
        wait_until(|| app.model().terminal_tabs.is_empty());
        app.widget().destroy();
    }
}

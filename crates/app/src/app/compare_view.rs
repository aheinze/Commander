//! Reviewed, virtualized folder synchronization with cancellable background work.

use super::*;
use crate::features::{
    CompareEntry, SyncActionKind, SyncDirection, SyncPlan, compare_directories,
    compare_with_contents, execute_sync_plan, plan_sync,
};

pub(super) struct Comparison {
    vfs: Arc<dyn Vfs>,
    engine: OperationEngine,
    left: VPath,
    right: VPath,
    input: relm4::Sender<AppMsg>,
    direction: gtk::DropDown,
    mirror: gtk::CheckButton,
    verify: gtk::CheckButton,
    refresh: gtk::Button,
    apply: gtk::Button,
    stop: gtk::Button,
    status: notifications::Feedback,
    route: gtk::Label,
    store: gio::ListStore,
    entries: RefCell<Option<Vec<CompareEntry>>>,
    plan: RefCell<Option<SyncPlan>>,
    cancel: RefCell<CancelToken>,
    closed: Cell<bool>,
    busy: Cell<bool>,
}

impl Drop for Comparison {
    fn drop(&mut self) {
        self.cancel.get_mut().cancel();
    }
}

pub(super) fn show(
    vfs: Arc<dyn Vfs>,
    engine: OperationEngine,
    left: VPath,
    right: VPath,
    input: relm4::Sender<AppMsg>,
) -> Option<Rc<Comparison>> {
    let parent = relm4::main_application()
        .active_window()
        .or_else(|| relm4::main_application().windows().first().cloned())?;
    let dialog = adw::Dialog::builder()
        .title("Compare and sync folders")
        .content_width(860)
        .content_height(640)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&chrome::dialog_header(&dialog));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_start(18);
    root.set_margin_end(18);
    root.set_margin_top(8);
    root.set_margin_bottom(18);
    let roots = gtk::Box::new(gtk::Orientation::Vertical, 6);
    for (name, path) in [("Left", &left), ("Right", &right)] {
        let label = gtk::Label::new(Some(&format!("{name}: {path}")));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_width_chars(1);
        label.set_tooltip_text(Some(&path.to_string()));
        label.set_selectable(true);
        roots.append(&label);
    }
    root.append(&roots);
    let options = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(1)
        .max_children_per_line(3)
        .column_spacing(12)
        .row_spacing(6)
        .build();
    let direction = gtk::DropDown::from_strings(&["Left → Right", "Right → Left"]);
    direction.set_tooltip_text(Some("Choose which folder supplies the changes"));
    let verify = gtk::CheckButton::with_label("Compare file contents");
    verify.set_tooltip_text(Some(
        "Read SHA-256 hashes, including files with matching size and timestamps",
    ));
    let mirror = gtk::CheckButton::with_label("Mirror destination");
    mirror.set_tooltip_text(Some(
        "Include destination-only items in the plan as Move to Trash",
    ));
    options.append(&direction);
    options.append(&verify);
    options.append(&mirror);
    root.append(&options);
    let route = gtk::Label::new(None);
    route.set_xalign(0.0);
    route.set_wrap(true);
    route.add_css_class("heading");
    root.append(&route);
    let status = notifications::Feedback::default();
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let list = gtk::ListView::new(
        Some(gtk::NoSelection::new(Some(store.clone()))),
        Some(action_factory()),
    );
    list.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    root.append(&scroll);
    let note = gtk::Label::new(Some(
        "Copied files are verified before replacement. Mirror removals go to Trash. Recovery backups and transfer temporary files are excluded. Resolve file/folder type conflicts before syncing.",
    ));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_max_width_chars(90);
    note.add_css_class("dim-label");
    root.append(&note);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let refresh = gtk::Button::with_label("Compare again");
    let stop = gtk::Button::with_label("Stop");
    let apply = gtk::Button::with_label("Apply reviewed changes");
    apply.add_css_class("suggested-action");
    apply.set_sensitive(false);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&refresh);
    footer.append(&stop);
    footer.append(&spacer);
    footer.append(&apply);
    root.append(&footer);
    view.set_content(Some(&root));
    dialog.set_child(Some(&view));
    let state = Rc::new(Comparison {
        vfs,
        engine,
        left,
        right,
        input,
        direction,
        mirror,
        verify,
        refresh,
        apply,
        stop,
        status,
        route,
        store,
        entries: RefCell::new(None),
        plan: RefCell::new(None),
        cancel: RefCell::new(CancelToken::new()),
        closed: Cell::new(false),
        busy: Cell::new(false),
    });
    {
        let button = &state.refresh;
        let weak = Rc::downgrade(&state);
        button.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.compare();
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        state.verify.connect_toggled(move |_| {
            if let Some(state) = weak.upgrade() {
                state.compare();
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        state.direction.connect_selected_notify(move |_| {
            if let Some(state) = weak.upgrade() {
                state.review();
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        state.mirror.connect_toggled(move |_| {
            if let Some(state) = weak.upgrade() {
                state.review();
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        state.stop.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.cancel.borrow().cancel();
                state
                    .status
                    .info("Stopping after the current filesystem call…");
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        state.apply.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.apply_plan();
            }
        });
    }
    {
        let state = Rc::clone(&state);
        dialog.connect_closed(move |_| {
            state.closed.set(true);
            state.cancel.borrow().cancel();
        });
    }
    dialog.present(Some(&parent));
    state.compare();
    Some(state)
}

impl Comparison {
    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.refresh.set_sensitive(!busy);
        self.verify.set_sensitive(!busy);
        self.direction.set_sensitive(!busy);
        self.mirror.set_sensitive(!busy);
        self.stop.set_visible(busy);
        self.apply.set_sensitive(false);
    }
    fn compare(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        self.set_busy(true);
        self.entries.replace(None);
        self.plan.replace(None);
        self.store.remove_all();
        let cancel = CancelToken::new();
        self.cancel.replace(cancel.clone());
        let verified = self.verify.is_active();
        self.status.info(if verified {
            "Comparing contents with SHA-256…"
        } else {
            "Comparing sizes and modification times…"
        });
        self.route
            .set_label("Review the planned changes before applying them");
        let vfs = Arc::clone(&self.vfs);
        let left = self.left.clone();
        let right = self.right.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = thread::Builder::new()
            .name("commander-compare".into())
            .spawn(move || {
                let _ = tx.send(if verified {
                    compare_with_contents(vfs.as_ref(), &left, &right, true, &cancel)
                } else {
                    compare_directories(vfs.as_ref(), &left, &right, &cancel)
                });
            });
        if let Err(error) = worker {
            self.set_busy(false);
            self.status
                .error(&format!("Could not start comparison: {error}"));
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(state) = weak.upgrade().filter(|state| !state.closed.get()) else {
                return glib::ControlFlow::Break;
            };
            match rx.try_recv() {
                Ok(result) => {
                    state.set_busy(false);
                    match result {
                        Ok(entries) => {
                            state.entries.replace(Some(entries));
                            state.review();
                        }
                        Err(error) => state.status.error(&error),
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => {
                    state.set_busy(false);
                    state
                        .status
                        .error("Comparison stopped unexpectedly. Compare again.");
                    glib::ControlFlow::Break
                }
            }
        });
    }
    fn review(&self) {
        if self.busy.get() {
            return;
        }
        let Some(entries) = self.entries.borrow().clone() else {
            return;
        };
        let direction = if self.direction.selected() == 0 {
            SyncDirection::LeftToRight
        } else {
            SyncDirection::RightToLeft
        };
        match plan_sync(
            self.left.clone(),
            self.right.clone(),
            entries,
            direction,
            self.mirror.is_active(),
            self.verify.is_active(),
        ) {
            Ok(plan) => {
                let source = if direction == SyncDirection::LeftToRight {
                    "Left"
                } else {
                    "Right"
                };
                let destination = if direction == SyncDirection::LeftToRight {
                    "Right"
                } else {
                    "Left"
                };
                self.route.set_label(&format!(
                    "{source} → {destination} · {}",
                    if self.mirror.is_active() {
                        "Mirror"
                    } else {
                        "Update"
                    }
                ));
                let basis = if plan.verified {
                    "Contents verified with SHA-256"
                } else {
                    "Compared sizes and modification times"
                };
                let copy = plan
                    .actions
                    .iter()
                    .filter(|action| {
                        matches!(
                            action.kind,
                            SyncActionKind::Copy | SyncActionKind::CreateDirectory
                        )
                    })
                    .count();
                let replace = plan
                    .actions
                    .iter()
                    .filter(|action| action.kind == SyncActionKind::Replace)
                    .count();
                let trash = plan
                    .actions
                    .iter()
                    .filter(|action| action.kind == SyncActionKind::Trash)
                    .count();
                self.status.info(&if plan.blocked() {
                    "Resolve the listed type conflicts or unsupported entries, then compare again."
                        .into()
                } else if plan.actions.is_empty() {
                    format!("No changes to apply in this direction. {basis}.")
                } else {
                    let replacement = if replace == 1 {
                        "replacement"
                    } else {
                        "replacements"
                    };
                    format!("{copy} new · {replace} {replacement} · {trash} to Trash. {basis}.")
                });
                let change = if plan.actions.len() == 1 {
                    "change"
                } else {
                    "changes"
                };
                self.apply
                    .set_label(&format!("Apply {} {change}", plan.actions.len()));
                self.apply
                    .set_sensitive(!plan.actions.is_empty() && !plan.blocked());
                let rows: Vec<_> = plan
                    .actions
                    .iter()
                    .cloned()
                    .map(glib::BoxedAnyObject::new)
                    .collect();
                self.store.splice(0, self.store.n_items(), &rows);
                self.plan.replace(Some(plan));
            }
            Err(error) => {
                self.plan.replace(None);
                self.apply.set_sensitive(false);
                self.status.error(&error);
            }
        }
    }
    fn apply_plan(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let Some(plan) = self
            .plan
            .borrow()
            .clone()
            .filter(|plan| !plan.actions.is_empty() && !plan.blocked())
        else {
            return;
        };
        self.set_busy(true);
        self.plan.replace(None);
        self.entries.replace(None);
        let cancel = CancelToken::new();
        self.cancel.replace(cancel.clone());
        let vfs = Arc::clone(&self.vfs);
        let engine = self.engine.clone();
        let input = self.input.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = thread::Builder::new()
            .name("commander-sync".into())
            .spawn(move || {
                let total = plan.actions.len();
                let result =
                    execute_sync_plan(vfs.as_ref(), &engine, &plan, &cancel, |done, message| {
                        let _ = tx.send(Err(format!("{done}/{total} · {message}")));
                    });
                let _ = input.send(AppMsg::SyncReady(result.clone()));
                let _ = tx.send(Ok(result));
            });
        if let Err(error) = worker {
            self.set_busy(false);
            self.status.error(&format!("Could not start sync: {error}"));
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(state) = weak.upgrade().filter(|state| !state.closed.get()) else {
                return glib::ControlFlow::Break;
            };
            loop {
                match rx.try_recv() {
                    Ok(Err(progress)) => state.stop.set_tooltip_text(Some(&progress)),
                    Ok(Ok(result)) => {
                        state.set_busy(false);
                        match result { Ok(count) => state.status.success(&format!("Applied {count} changes. Compare again to review the updated folders.")), Err(error) => state.status.error(&error) };
                        return glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(_) => {
                        state.set_busy(false);
                        state
                            .status
                            .error("Sync stopped unexpectedly. Compare again before retrying.");
                        return glib::ControlFlow::Break;
                    }
                }
            }
        });
    }
}

fn action_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, object| {
        let item = object.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_margin_start(12);
        row.set_margin_end(12);
        row.set_margin_top(8);
        row.set_margin_bottom(8);
        let action = gtk::Label::new(None);
        action.set_width_chars(16);
        action.set_xalign(0.0);
        let path = gtk::Label::new(None);
        path.set_xalign(0.0);
        path.set_hexpand(true);
        path.set_width_chars(1);
        path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let size = gtk::Label::new(None);
        size.add_css_class("dim-label");
        row.append(&action);
        row.append(&path);
        row.append(&size);
        item.set_child(Some(&row));
        item.connect_notify_local(Some("item"), move |item, _| {
            let Some(value) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let entry = value.borrow::<crate::features::SyncAction>();
            action.set_label(entry.kind.label());
            for class in ["error", "warning", "accent"] {
                action.remove_css_class(class);
            }
            action.add_css_class(match entry.kind {
                SyncActionKind::Blocked => "error",
                SyncActionKind::Trash | SyncActionKind::Replace => "warning",
                _ => "accent",
            });
            path.set_label(&entry.relative_path.to_string_lossy());
            row.set_tooltip_text(Some(&entry.relative_path.to_string_lossy()));
            size.set_label(&format!(
                "{} → {}",
                entry
                    .source
                    .as_ref()
                    .map_or("—".into(), |meta| format_size(meta.size, meta.kind)),
                entry
                    .destination
                    .as_ref()
                    .map_or("—".into(), |meta| format_size(meta.size, meta.kind))
            ));
        });
    });
    factory
}

#[cfg(test)]
#[path = "compare_view_tests.rs"]
mod tests;

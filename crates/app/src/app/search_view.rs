//! The recursive search panel.

use super::*;

#[cfg(test)]
#[path = "search_view_tests.rs"]
mod tests;

pub(super) struct SearchWidgets {
    pub(super) dialog: adw::Dialog,
    pub(super) parent: adw::ApplicationWindow,
    pub(super) query: gtk::SearchEntry,
    pub(super) content: gtk::ToggleButton,
    pub(super) status: gtk::Label,
    pub(super) results: gtk::ListView,
    pub(super) result_store: gio::ListStore,
    pub(super) stop: gtk::Button,
    pub(super) rendered_open: Rc<Cell<bool>>,
    closing_from_model: Rc<Cell<bool>>,
    pub(super) rendered_preset: Cell<bool>,
    pub(super) rendered_results: RefCell<Vec<SearchHit>>,
}

/// A pill toggle used for the search facets.
fn facet_chip(label: &str, tooltip: &str) -> gtk::ToggleButton {
    let chip = gtk::ToggleButton::with_label(label);
    chip.add_css_class("search-chip");
    chip.set_tooltip_text(Some(tooltip));
    chip
}

/// A small muted caption in front of a filter control.
fn filter_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("search-facet-label");
    label
}

impl SearchWidgets {
    pub(super) fn new(parent: &adw::ApplicationWindow, sender: &ComponentSender<AppModel>) -> Self {
        let rendered_open = Rc::new(Cell::new(false));
        let closing_from_model = Rc::new(Cell::new(false));
        let dialog = adw::Dialog::builder()
            .title("Search")
            .content_width(760)
            .content_height(600)
            .presentation_mode(adw::DialogPresentationMode::Floating)
            .build();
        dialog.add_css_class("utility-dialog");
        dialog.add_css_class("search-dialog");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("search-sheet");

        let query_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        query_row.add_css_class("dialog-search-bar");
        let query = gtk::SearchEntry::new();
        query.set_hexpand(true);
        query.set_placeholder_text(Some("Search names, paths, and file contents…"));
        query.add_css_class("dialog-search");
        let search = gtk::Button::with_label("Search");
        search.add_css_class("search-run");
        search.set_valign(gtk::Align::Center);
        query_row.append(&query);
        query_row.append(&search);
        let stop = gtk::Button::with_label("Stop");
        stop.set_tooltip_text(Some("Stop the current search"));
        stop.set_visible(false);
        {
            let input = sender.input_sender().clone();
            stop.connect_clicked(move |_| {
                let _ = input.send(AppMsg::CancelSearch);
            });
        }
        query_row.append(&stop);
        root.append(&query_row);

        let facets = facet_flow();
        facets.add_css_class("search-facets");
        let content = facet_chip("Contents", "Also search inside file contents");
        let hidden = facet_chip("Hidden", "Include hidden files and folders");
        let case_sensitive = facet_chip("Match case", "Case-sensitive matching");
        let regex = facet_chip("Regex", "Treat the query as a regular expression");
        let symlinks = facet_chip(
            "Symlinks",
            "Include symbolic link entries without following linked folders",
        );
        symlinks.set_active(true);
        let ignored = facet_chip("Ignored", "Include ignored and build folders");
        for chip in [
            &content,
            &hidden,
            &case_sensitive,
            &regex,
            &symlinks,
            &ignored,
        ] {
            facets.append(chip);
        }
        let filters = facet_chip("Filters", "Type, size, date, and depth limits");
        filters.add_css_class("search-chip-filters");
        facets.append(&filters);
        root.append(&facets);

        let advanced = gtk::Box::new(gtk::Orientation::Vertical, 8);
        advanced.add_css_class("search-advanced");
        advanced.set_visible(false);
        let row_one = facet_flow();
        let kind = gtk::DropDown::from_strings(&["Any type", "Files", "Folders", "Links"]);
        kind.set_tooltip_text(Some("Limit results by item type"));
        let extension = gtk::Entry::new();
        extension.set_placeholder_text(Some("Extension"));
        extension.set_width_chars(9);
        let min_size = gtk::SpinButton::with_range(0.0, 1_000_000.0, 1.0);
        min_size.set_width_chars(4);
        min_size.set_tooltip_text(Some("Minimum size in MiB · 0 means any"));
        let max_size = gtk::SpinButton::with_range(0.0, 1_000_000.0, 1.0);
        max_size.set_width_chars(4);
        max_size.set_tooltip_text(Some("Maximum size in MiB · 0 means any"));
        let modified = gtk::DropDown::from_strings(&[
            "Any date",
            "Today",
            "Last 7 days",
            "Last 30 days",
            "Last year",
        ]);
        row_one.append(&kind);
        row_one.append(&extension);
        row_one.append(&filter_field("Min MiB", &min_size));
        row_one.append(&filter_field("Max MiB", &max_size));
        row_one.append(&modified);
        advanced.append(&row_one);
        let row_two = facet_flow();
        let depth = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
        depth.set_width_chars(3);
        depth.set_value(0.0);
        depth.set_tooltip_text(Some("0 searches every subfolder"));
        let content_limit =
            gtk::SpinButton::with_range(1.0, (MAX_CONTENT_BYTES / 1_048_576) as f64, 1.0);
        content_limit.set_width_chars(4);
        content_limit.set_value(8.0);
        content_limit.set_tooltip_text(Some("Maximum file size read during content search"));
        row_two.append(&filter_field("Depth", &depth));
        row_two.append(&filter_field("Content limit (MiB/file)", &content_limit));
        advanced.append(&row_two);
        root.append(&advanced);
        {
            let advanced = advanced.clone();
            filters.connect_toggled(move |chip| advanced.set_visible(chip.is_active()));
        }

        let status = gtk::Label::new(Some("0 results"));
        status.add_css_class("search-status");
        status.set_xalign(0.0);
        root.append(&status);
        let result_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let results = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(result_store.clone()))),
            Some(result_factory()),
        );
        results.set_single_click_activate(true);
        results.add_css_class("search-results");
        results.add_css_class("navigation-sidebar");
        {
            let input = sender.input_sender().clone();
            let result_store = result_store.clone();
            results.connect_activate(move |_, position| {
                if let Some(item) = result_store
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
                {
                    let _ = input.send(AppMsg::OpenSearchResult(
                        item.borrow::<SearchHit>().path.clone(),
                    ));
                }
            });
        }
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&results)
            .build();
        root.append(&scrolled);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&chrome::dialog_header(&dialog));
        view.set_content(Some(&root));
        dialog.set_child(Some(&view));

        let send_search = {
            let input = sender.input_sender().clone();
            let query = query.clone();
            let content = content.clone();
            let hidden = hidden.clone();
            let depth = depth.clone();
            let case_sensitive = case_sensitive.clone();
            let regex = regex.clone();
            let symlinks = symlinks.clone();
            let ignored = ignored.clone();
            let kind = kind.clone();
            let extension = extension.clone();
            let min_size = min_size.clone();
            let max_size = max_size.clone();
            let modified = modified.clone();
            let content_limit = content_limit.clone();
            move || {
                let depth_value = depth.value_as_int();
                let min_mib = u64::try_from(min_size.value_as_int()).unwrap_or(0);
                let max_mib = u64::try_from(max_size.value_as_int()).unwrap_or(0);
                let _ = input.send(AppMsg::RunSearch(SearchOptions {
                    query: query.text().to_string(),
                    search_content: content.is_active(),
                    include_hidden: hidden.is_active(),
                    max_depth: (depth_value > 0).then_some(depth_value as usize),
                    case_sensitive: case_sensitive.is_active(),
                    regex: regex.is_active(),
                    extension: (!extension.text().trim().is_empty())
                        .then(|| extension.text().trim().to_owned()),
                    kind: match kind.selected() {
                        1 => Some(EntryKind::File),
                        2 => Some(EntryKind::Directory),
                        3 => Some(EntryKind::Symlink),
                        _ => None,
                    },
                    min_size: (min_mib > 0).then_some(min_mib.saturating_mul(1_048_576)),
                    max_size: (max_mib > 0).then_some(max_mib.saturating_mul(1_048_576)),
                    modified_after: match modified.selected() {
                        1 => Some(1),
                        2 => Some(7),
                        3 => Some(30),
                        4 => Some(365),
                        _ => None,
                    }
                    .map(|days: i64| {
                        glib::DateTime::now_local()
                            .ok()
                            .map_or(0, |now| now.to_unix())
                            .saturating_sub(days.saturating_mul(86_400))
                    }),
                    include_symlinks: symlinks.is_active(),
                    include_ignored: ignored.is_active(),
                    max_content_bytes: u64::try_from(content_limit.value_as_int())
                        .unwrap_or(8)
                        .saturating_mul(1_048_576),
                }));
            }
        };
        {
            let send_search = send_search.clone();
            search.connect_clicked(move |_| send_search());
        }
        query.connect_activate(move |_| send_search());
        {
            let dialog = dialog.clone();
            query.connect_stop_search(move |_| {
                dialog.close();
            });
        }
        {
            let input = sender.input_sender().clone();
            let rendered_open = Rc::clone(&rendered_open);
            let closing_from_model = Rc::clone(&closing_from_model);
            let parent = parent.downgrade();
            dialog.connect_closed(move |dialog| {
                if closing_from_model.replace(false) {
                    // Finish the old close before honoring a rapid reopen.
                    // Otherwise the old animation can close the new dialog.
                    let dialog = dialog.downgrade();
                    let parent = parent.clone();
                    let rendered_open = Rc::clone(&rendered_open);
                    let closing_from_model = Rc::clone(&closing_from_model);
                    glib::idle_add_local_once(move || {
                        if rendered_open.get()
                            && !closing_from_model.get()
                            && let (Some(dialog), Some(parent)) =
                                (dialog.upgrade(), parent.upgrade())
                        {
                            dialog.present(Some(&parent));
                        }
                    });
                } else {
                    rendered_open.set(false);
                    let _ = input.send(AppMsg::CloseSearch);
                }
            });
        }

        Self {
            dialog,
            parent: parent.clone(),
            query,
            content,
            status,
            results,
            result_store,
            stop,
            rendered_open,
            closing_from_model,
            rendered_preset: Cell::new(false),
            rendered_results: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn render(&self, model: &AppModel, _sender: &ComponentSender<AppModel>) {
        if self.rendered_open.replace(model.search_open) != model.search_open {
            if model.search_open {
                if !self.closing_from_model.get() {
                    self.dialog.present(Some(&self.parent));
                }
                self.query.grab_focus();
            } else if !self.closing_from_model.get() {
                self.closing_from_model.set(true);
                if !self.dialog.close() {
                    self.closing_from_model.set(false);
                }
            }
        }
        if !model.search_open {
            return;
        }
        if self.rendered_preset.replace(model.search_content_preset) != model.search_content_preset
        {
            self.content.set_active(model.search_content_preset);
        }
        self.stop.set_visible(model.search_loading);
        let report = &model.search_results;
        self.status.set_label(&if model.search_loading {
            format!("Searching… {} results", report.hits.len())
        } else {
            format!("{} results", report.hits.len())
        });
        let feedback = if model.search_loading
            || (model.search_generation == 0
                && report.hits.is_empty()
                && model.search_error.is_none()
                && report.first_error.is_none())
        {
            None
        } else {
            Some(model.search_error.clone().unwrap_or_else(|| {
                let summary = report.summary();
                report
                    .first_error
                    .as_ref()
                    .map_or(summary.clone(), |error| format!("{summary}\n{error}"))
            }))
        };
        notifications::observe(
            "search",
            feedback.as_deref(),
            if model.search_error.is_some() || report.first_error.is_some() {
                notifications::Kind::Error
            } else {
                notifications::Kind::Info
            },
        );
        if *self.rendered_results.borrow() != report.hits {
            let items: Vec<_> = report
                .hits
                .iter()
                .cloned()
                .map(glib::BoxedAnyObject::new)
                .collect();
            self.result_store
                .splice(0, self.result_store.n_items(), &items);
            self.rendered_results.replace(report.hits.clone());
            if let Some(adjustment) = self.results.vadjustment() {
                adjustment.set_value(0.0);
            }
        }
    }
}

fn facet_flow() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(1)
        .max_children_per_line(7)
        .row_spacing(6)
        .column_spacing(6)
        .build()
}

fn filter_field(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let field = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    field.append(&filter_label(label));
    field.append(control);
    field
}

fn result_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, object| {
        let item = object.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("search-result-row");
        let icon = gtk::Image::new();
        icon.set_pixel_size(20);
        icon.add_css_class("search-result-icon");
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_width_chars(1);
        name.add_css_class("search-result-name");
        let path = gtk::Label::new(None);
        path.set_xalign(0.0);
        path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        path.set_width_chars(1);
        path.add_css_class("search-result-path");
        labels.append(&name);
        labels.append(&path);
        let detail = gtk::Label::new(None);
        detail.add_css_class("search-result-detail");
        row.append(&icon);
        row.append(&labels);
        row.append(&detail);
        item.set_child(Some(&row));
        item.connect_notify_local(Some("item"), move |item, _| {
            let Some(value) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let hit = value.borrow::<SearchHit>();
            let file_name = hit.path.file_name().unwrap_or_default();
            crate::icons::set_file_icon(&icon, hit.kind, file_name);
            name.set_label(&file_name.to_string_lossy());
            path.set_label(&hit.path.to_string());
            row.set_tooltip_text(Some(&format!("{}\nShow in containing folder", hit.path)));
            detail.set_label(&format!(
                "{}{}",
                format_size(hit.size, hit.kind),
                if hit.content_match { " · content" } else { "" }
            ));
        });
    });
    factory
}

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
    pub(super) results: gtk::Box,
    pub(super) rendered_open: Cell<bool>,
    pub(super) rendered_preset: Cell<bool>,
    pub(super) rendered_results: RefCell<Vec<String>>,
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
        root.append(&query_row);

        let facets = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        facets.add_css_class("search-facets");
        let content = facet_chip("Contents", "Also search inside file contents");
        let hidden = facet_chip("Hidden", "Include hidden files and folders");
        let case_sensitive = facet_chip("Match case", "Case-sensitive matching");
        let regex = facet_chip("Regex", "Treat the query as a regular expression");
        let symlinks = facet_chip("Symlinks", "Follow symbolic links");
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
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        facets.append(&spacer);
        let filters = facet_chip("Filters", "Type, size, date, and depth limits");
        filters.add_css_class("search-chip-filters");
        facets.append(&filters);
        root.append(&facets);

        let advanced = gtk::Box::new(gtk::Orientation::Vertical, 8);
        advanced.add_css_class("search-advanced");
        advanced.set_visible(false);
        let row_one = gtk::Box::new(gtk::Orientation::Horizontal, 8);
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
        row_one.append(&filter_label("Min MiB"));
        row_one.append(&min_size);
        row_one.append(&filter_label("Max MiB"));
        row_one.append(&max_size);
        row_one.append(&modified);
        advanced.append(&row_one);
        let row_two = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let depth = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
        depth.set_width_chars(3);
        depth.set_value(0.0);
        depth.set_tooltip_text(Some("0 searches every subfolder"));
        let content_limit = gtk::SpinButton::with_range(1.0, 1_024.0, 1.0);
        content_limit.set_width_chars(4);
        content_limit.set_value(8.0);
        content_limit.set_tooltip_text(Some("Maximum file size read during content search"));
        row_two.append(&filter_label("Depth"));
        row_two.append(&depth);
        row_two.append(&filter_label("Content scan limit"));
        row_two.append(&content_limit);
        row_two.append(&filter_label("MiB per file"));
        advanced.append(&row_two);
        root.append(&advanced);
        {
            let advanced = advanced.clone();
            filters.connect_toggled(move |chip| advanced.set_visible(chip.is_active()));
        }

        let status = gtk::Label::new(Some("Press Enter to search the active folder"));
        status.add_css_class("search-status");
        status.set_xalign(0.0);
        root.append(&status);
        let results = gtk::Box::new(gtk::Orientation::Vertical, 1);
        results.add_css_class("search-results");
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&results)
            .build();
        root.append(&scrolled);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
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
            dialog.connect_closed(move |_| {
                let _ = input.send(AppMsg::CloseSearch);
            });
        }

        Self {
            dialog,
            parent: parent.clone(),
            query,
            content,
            status,
            results,
            rendered_open: Cell::new(false),
            rendered_preset: Cell::new(false),
            rendered_results: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn render(&self, model: &AppModel, sender: &ComponentSender<AppModel>) {
        if self.rendered_open.replace(model.search_open) != model.search_open {
            if model.search_open {
                self.dialog.present(Some(&self.parent));
                self.query.grab_focus();
            } else {
                self.dialog.close();
            }
        }
        if !model.search_open {
            return;
        }
        if self.rendered_preset.replace(model.search_content_preset) != model.search_content_preset
        {
            self.content.set_active(model.search_content_preset);
        }
        self.status.set_label(if model.search_loading {
            "Searching…"
        } else if let Some(error) = &model.search_error {
            error
        } else if model.search_results.is_empty() {
            "No matching files"
        } else {
            return self.render_results(model, sender);
        });
        if model.search_loading || model.search_results.is_empty() {
            while let Some(child) = self.results.first_child() {
                self.results.remove(&child);
            }
            self.rendered_results.borrow_mut().clear();
        }
    }

    pub(super) fn render_results(&self, model: &AppModel, sender: &ComponentSender<AppModel>) {
        self.status
            .set_label(&format!("{} results", model.search_results.len()));
        let signature: Vec<_> = model
            .search_results
            .iter()
            .map(|hit| format!("{}:{}", hit.path, hit.content_match))
            .collect();
        if *self.rendered_results.borrow() == signature {
            return;
        }
        *self.rendered_results.borrow_mut() = signature;
        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        for hit in model.search_results.iter().take(2_000) {
            let row = gtk::Button::new();
            row.add_css_class("flat");
            row.add_css_class("search-result-row");
            row.set_tooltip_text(Some("Show in containing folder"));
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let icon = gtk::Image::new();
            crate::icons::set_file_icon(&icon, hit.kind, hit.path.file_name().unwrap_or_default());
            icon.set_pixel_size(16);
            icon.add_css_class("search-result-icon");
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
            labels.set_hexpand(true);
            let name = gtk::Label::new(Some(
                hit.path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("Item"),
            ));
            name.set_xalign(0.0);
            name.add_css_class("search-result-name");
            let path = gtk::Label::new(Some(&hit.path.to_string()));
            path.set_xalign(0.0);
            path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            path.add_css_class("search-result-path");
            labels.append(&name);
            labels.append(&path);
            let detail = gtk::Label::new(Some(&format!(
                "{}{}",
                format_size(hit.size, hit.kind),
                if hit.content_match { " · content" } else { "" }
            )));
            detail.add_css_class("search-result-detail");
            content.append(&icon);
            content.append(&labels);
            content.append(&detail);
            row.set_child(Some(&content));
            let input = sender.input_sender().clone();
            let target = hit.path.clone();
            row.connect_clicked(move |_| {
                let _ = input.send(AppMsg::OpenSearchResult(target.clone()));
            });
            self.results.append(&row);
        }
    }
}

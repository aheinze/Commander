//! The inspector preview pane and the Quick Look window.

use super::*;

mod text;

pub(super) struct QuickLookWidgets {
    pub(super) dialog: adw::Dialog,
    pub(super) parent: adw::ApplicationWindow,
    pub(super) stack: gtk::Stack,
    pub(super) icon: gtk::Image,
    pub(super) picture: gtk::Picture,
    pub(super) video: gtk::Video,
    pub(super) text_view: gtk::TextView,
    pub(super) status: gtk::Label,
    pub(super) title: gtk::Label,
    pub(super) detail: gtk::Label,
    pub(super) rendered_preview: Cell<(u64, bool)>,
    pub(super) rendered_open: Rc<Cell<bool>>,
}

impl QuickLookWidgets {
    pub(super) fn new(parent: &adw::ApplicationWindow, sender: &ComponentSender<AppModel>) -> Self {
        let dialog = adw::Dialog::builder()
            .title("Quick Look")
            .content_width(980)
            .content_height(720)
            .presentation_mode(adw::DialogPresentationMode::Floating)
            .build();
        dialog.add_css_class("utility-dialog");
        dialog.add_css_class("quick-look-dialog");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("quick-look-root");
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 1);
        heading.add_css_class("quick-look-heading");
        heading.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some("Quick Look"));
        title.add_css_class("quick-look-title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        title.set_max_width_chars(60);
        let detail = gtk::Label::new(Some("Space to close · ←/→ to browse"));
        detail.add_css_class("quick-look-detail");
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        heading.append(&title);
        heading.append(&detail);
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&heading));

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(140);
        let icon = gtk::Image::from_icon_name("commander-file-symbolic");
        icon.set_pixel_size(160);
        stack.add_named(&icon, Some("icon"));
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        stack.add_named(&picture, Some("image"));
        let video = gtk::Video::new();
        video.set_autoplay(true);
        video.set_loop(false);
        stack.add_named(&video, Some("media"));
        let text_view = text::new_view("quick-look-text");
        let text_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&text_view)
            .build();
        stack.add_named(&text_scroll, Some("text"));
        let loading = gtk::Spinner::new();
        loading.set_spinning(true);
        stack.add_named(&loading, Some("loading"));
        let status = gtk::Label::new(None);
        status.set_wrap(true);
        status.set_justify(gtk::Justification::Center);
        status.add_css_class("quick-look-status");
        stack.add_named(&status, Some("status"));
        root.append(&stack);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&root));
        dialog.set_child(Some(&view));

        let rendered_open = Rc::new(Cell::new(false));
        {
            // Escape and the close button are handled by the sheet itself; the
            // model only needs to hear about it when the sheet closed on its own.
            let input = sender.input_sender().clone();
            let rendered_open = Rc::clone(&rendered_open);
            dialog.connect_closed(move |_| {
                if rendered_open.replace(false) {
                    let _ = input.send(AppMsg::ToggleQuickLook);
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        {
            let input = sender.input_sender().clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::space {
                    let _ = input.send(AppMsg::ToggleQuickLook);
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Left || key == gdk::Key::Up {
                    let _ = input.send(AppMsg::MoveCursor(-1, false));
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Right || key == gdk::Key::Down {
                    let _ = input.send(AppMsg::MoveCursor(1, false));
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        dialog.add_controller(keys);

        Self {
            dialog,
            parent: parent.clone(),
            stack,
            icon,
            picture,
            video,
            text_view,
            status,
            title,
            detail,
            rendered_preview: Cell::new((u64::MAX, false)),
            rendered_open,
        }
    }

    pub(super) fn render(&self, model: &AppModel) {
        if self.rendered_open.replace(model.quick_look_open) != model.quick_look_open {
            if model.quick_look_open {
                self.dialog.present(Some(&self.parent));
            } else {
                self.dialog.close();
            }
        }
        if !model.quick_look_open {
            return;
        }
        let state = &model.preview_state;
        let key = (state.generation, state.loading);
        if self.rendered_preview.get() == key {
            return;
        }
        self.rendered_preview.set(key);
        self.title.set_label(
            state
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(OsStr::to_str)
                .unwrap_or("Quick Look"),
        );
        if state.loading {
            self.detail.set_label("Loading preview…");
            self.stack.set_visible_child_name("loading");
            return;
        }
        if let Some(error) = &state.error {
            self.status.set_label(error);
            self.stack.set_visible_child_name("status");
            return;
        }
        let Some(preview) = &state.content else {
            self.status.set_label("No item is focused");
            self.stack.set_visible_child_name("status");
            return;
        };
        self.detail.set_label(&format!(
            "{} · Space to close · ←/→ to browse",
            format_size(preview.metadata.size, preview.metadata.kind)
        ));
        match &preview.payload {
            PreviewPayload::Directory => {
                pause_video(&self.video);
                crate::icons::set_file_icon(
                    &self.icon,
                    preview.metadata.kind,
                    preview.path.file_name().unwrap_or_default(),
                );
                self.stack.set_visible_child_name("icon");
            }
            PreviewPayload::Image {
                rgba,
                width,
                height,
            } => {
                pause_video(&self.video);
                let (Ok(width), Ok(height)) = (i32::try_from(*width), i32::try_from(*height))
                else {
                    self.status.set_label("Image dimensions are too large");
                    self.stack.set_visible_child_name("status");
                    return;
                };
                let bytes = glib::Bytes::from_owned(rgba.clone());
                let texture = gdk::MemoryTexture::new(
                    width,
                    height,
                    gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    usize::try_from(width).unwrap_or(0).saturating_mul(4),
                );
                self.picture.set_paintable(Some(&texture));
                self.stack.set_visible_child_name("image");
            }
            PreviewPayload::Pdf {
                rgba,
                width,
                height,
                page_count,
                ..
            } => {
                pause_video(&self.video);
                if set_picture_rgba(&self.picture, rgba, *width, *height) {
                    self.detail.set_label(&format!(
                        "PDF · {page_count} page{} · Space to close · ←/→ to browse",
                        if *page_count == 1 { "" } else { "s" }
                    ));
                    self.stack.set_visible_child_name("image");
                } else {
                    self.status.set_label("PDF page is too large to preview");
                    self.stack.set_visible_child_name("status");
                }
            }
            PreviewPayload::Text {
                content,
                language,
                truncated,
                highlights,
            } => {
                pause_video(&self.video);
                text::render(&self.text_view, content, highlights);
                self.detail.set_label(&format!(
                    "{language}{} · Space to close · ←/→ to browse",
                    if *truncated { " · truncated" } else { "" }
                ));
                self.stack.set_visible_child_name("text");
            }
            PreviewPayload::Media { bytes, kind } => {
                set_video_bytes(&self.video, bytes);
                self.detail
                    .set_label(&format!("{kind} · Space to close · ←/→ to browse"));
                self.stack.set_visible_child_name("media");
            }
            PreviewPayload::Unsupported => {
                pause_video(&self.video);
                crate::icons::set_file_icon(
                    &self.icon,
                    preview.metadata.kind,
                    preview.path.file_name().unwrap_or_default(),
                );
                self.stack.set_visible_child_name("icon");
            }
        }
    }
}

pub(super) struct PreviewWidgets {
    pub(super) root: gtk::Revealer,
    pub(super) stack: gtk::Stack,
    pub(super) tab_title: gtk::Label,
    pub(super) tabs: [gtk::ToggleButton; 3],
    pub(super) content_stack: gtk::Stack,
    pub(super) icon: gtk::Image,
    pub(super) picture: gtk::Picture,
    pub(super) pdf_picture: gtk::Picture,
    pub(super) video: gtk::Video,
    pub(super) text_view: gtk::TextView,
    pub(super) content_status: gtk::Label,
    pub(super) pdf_controls: gtk::Box,
    pub(super) pdf_previous: gtk::Button,
    pub(super) pdf_next: gtk::Button,
    pub(super) pdf_page: gtk::Label,
    pub(super) pdf_zoom: gtk::Label,
    pub(super) rendered_preview: Cell<(u64, bool, usize)>,
    pub(super) identity: gtk::Box,
    pub(super) name: gtk::Label,
    pub(super) meta: gtk::Label,
    pub(super) details_section: gtk::Box,
    pub(super) detail_type_value: gtk::Label,
    pub(super) detail_dimensions_value: gtk::Label,
    pub(super) path_value: gtk::Label,
    pub(super) size_value: gtk::Label,
    pub(super) modified_value: gtk::Label,
    pub(super) created_value: gtk::Label,
    pub(super) accessed_value: gtk::Label,
    pub(super) extension_value: gtk::Label,
    pub(super) hidden_value: gtk::Label,
    pub(super) read_only_value: gtk::Label,
    pub(super) general_section: gtk::Box,
    pub(super) permissions_section: gtk::Box,
    pub(super) permission_owner_value: gtk::Label,
    pub(super) permission_group_value: gtk::Label,
    pub(super) permission_others_value: gtk::Label,
    pub(super) permission_mode_value: gtk::Label,
    pub(super) permission_octal_value: gtk::Label,
    pub(super) permission_identity_value: gtk::Label,
    pub(super) git_section: gtk::Box,
    pub(super) git_state_pill: gtk::Label,
    pub(super) git_branch_value: gtk::Label,
    pub(super) git_upstream_value: gtk::Label,
    pub(super) git_item_value: gtk::Label,
    pub(super) git_tree_value: gtk::Label,
    pub(super) git_commit_row: gtk::Box,
    pub(super) git_commit_subject: gtk::Label,
    pub(super) git_commit_hash: gtk::Label,
    pub(super) git_commit_meta: gtk::Label,
    pub(super) git_root_value: gtk::Label,
    pub(super) work_summary: gtk::Label,
    pub(super) work_empty: gtk::Box,
    pub(super) work_scroll: gtk::ScrolledWindow,
    pub(super) work_list: job_view::JobListWidgets,
    pub(super) log_list: gtk::Box,
    pub(super) log_controls: gtk::Box,
    pub(super) log_empty: gtk::Box,
    pub(super) log_scroll: gtk::ScrolledWindow,
    pub(super) log_clear: gtk::Button,
    pub(super) rendered_log_revision: Cell<u64>,
}

impl PreviewWidgets {
    pub(super) fn new(sender: &ComponentSender<AppModel>) -> Self {
        let root = gtk::Revealer::new();
        root.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        root.set_transition_duration(160);
        root.set_vexpand(true);

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("preview-panel");
        panel.set_width_request(PREVIEW_MIN_WIDTH);

        let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        tab_bar.add_css_class("inspector-tabs");
        tab_bar.set_halign(gtk::Align::Fill);
        tab_bar.set_hexpand(true);
        let tab_title = gtk::Label::new(Some("Inspector"));
        tab_title.set_xalign(0.0);
        tab_title.set_hexpand(true);
        tab_title.add_css_class("inspector-tabs-title");
        tab_bar.append(&tab_title);
        let info_tab = view_toggle_button("commander-info-symbolic", "Info");
        let work_tab = view_toggle_button("commander-refresh-cw-symbolic", "Current work");
        let log_tab = view_toggle_button("commander-file-symbolic", "Activity log");
        work_tab.set_group(Some(&info_tab));
        log_tab.set_group(Some(&info_tab));
        for (button, page) in [
            (&info_tab, InspectorPage::Info),
            (&work_tab, InspectorPage::Work),
            (&log_tab, InspectorPage::Log),
        ] {
            button.add_css_class("flat");
            button.add_css_class("inspector-tab");
            let input = sender.input_sender().clone();
            button.connect_clicked(move |_| {
                let _ = input.send(AppMsg::SetInspectorPage(page));
            });
            tab_bar.append(button);
        }
        panel.append(&tab_bar);

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_vhomogeneous(false);

        let info_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        info_box.add_css_class("inspector-content");
        let hero = gtk::Box::new(gtk::Orientation::Vertical, 0);
        hero.add_css_class("preview-hero");
        let content_stack = gtk::Stack::new();
        content_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        content_stack.set_transition_duration(120);
        content_stack.set_height_request(220);
        let icon = gtk::Image::from_icon_name("commander-folder-symbolic");
        icon.set_pixel_size(82);
        content_stack.add_named(&icon, Some("icon"));
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        content_stack.add_named(&picture, Some("image"));
        let pdf_picture = gtk::Picture::new();
        pdf_picture.set_can_shrink(false);
        pdf_picture.set_content_fit(gtk::ContentFit::Contain);
        pdf_picture.set_halign(gtk::Align::Start);
        pdf_picture.set_valign(gtk::Align::Start);
        let pdf_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&pdf_picture)
            .build();
        pdf_scroll.add_css_class("pdf-preview-scroll");
        content_stack.add_named(&pdf_scroll, Some("pdf"));
        let video = gtk::Video::new();
        video.set_autoplay(true);
        video.set_loop(false);
        content_stack.add_named(&video, Some("media"));
        let text_view = text::new_view("preview-text");
        let text_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&text_view)
            .build();
        content_stack.add_named(&text_scroll, Some("text"));
        let loading = gtk::Spinner::new();
        loading.set_spinning(true);
        content_stack.add_named(&loading, Some("loading"));
        let content_status = gtk::Label::new(None);
        content_status.set_wrap(true);
        content_status.set_justify(gtk::Justification::Center);
        content_status.add_css_class("preview-content-status");
        content_stack.add_named(&content_status, Some("status"));
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.add_css_class("inspector-selection-empty");
        let empty_icon = gtk::Image::from_icon_name("commander-file-symbolic");
        empty_icon.set_pixel_size(42);
        let empty_title = gtk::Label::new(Some("Nothing selected"));
        empty_title.add_css_class("inspector-empty-title");
        let empty_copy = gtk::Label::new(Some("Select a file or folder to inspect"));
        empty_copy.add_css_class("inspector-empty-copy");
        empty.append(&empty_icon);
        empty.append(&empty_title);
        empty.append(&empty_copy);
        content_stack.add_named(&empty, Some("empty"));
        let multi = gtk::Box::new(gtk::Orientation::Vertical, 0);
        multi.set_halign(gtk::Align::Center);
        multi.set_valign(gtk::Align::Center);
        multi.add_css_class("selection-stack-art");
        let multi_icon = gtk::Image::from_icon_name("commander-layout-grid-symbolic");
        multi_icon.set_pixel_size(72);
        multi.append(&multi_icon);
        content_stack.add_named(&multi, Some("multi"));
        hero.append(&content_stack);
        let pdf_controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        pdf_controls.set_halign(gtk::Align::Center);
        pdf_controls.add_css_class("pdf-preview-controls");
        pdf_controls.set_visible(false);
        let pdf_previous = gtk::Button::from_icon_name("commander-chevron-left-symbolic");
        pdf_previous.set_tooltip_text(Some("Previous PDF page"));
        pdf_previous.add_css_class("pdf-preview-button");
        connect_button(&pdf_previous, sender, || AppMsg::PdfNavigate(-1));
        let pdf_page = gtk::Label::new(Some("1 / 1"));
        pdf_page.add_css_class("pdf-preview-page");
        let pdf_next = gtk::Button::from_icon_name("commander-chevron-right-symbolic");
        pdf_next.set_tooltip_text(Some("Next PDF page"));
        pdf_next.add_css_class("pdf-preview-button");
        connect_button(&pdf_next, sender, || AppMsg::PdfNavigate(1));
        let zoom_out = gtk::Button::from_icon_name("commander-zoom-out-symbolic");
        zoom_out.set_tooltip_text(Some("Zoom out"));
        zoom_out.add_css_class("pdf-preview-button");
        connect_button(&zoom_out, sender, || AppMsg::PdfZoom(-1));
        let zoom_fit = gtk::Button::from_icon_name("commander-scan-symbolic");
        zoom_fit.set_tooltip_text(Some("Fit page"));
        zoom_fit.add_css_class("pdf-preview-button");
        connect_button(&zoom_fit, sender, || AppMsg::PdfFit);
        let pdf_zoom = gtk::Label::new(Some("100%"));
        pdf_zoom.add_css_class("pdf-preview-page");
        let zoom_in = gtk::Button::from_icon_name("commander-zoom-in-symbolic");
        zoom_in.set_tooltip_text(Some("Zoom in"));
        zoom_in.add_css_class("pdf-preview-button");
        connect_button(&zoom_in, sender, || AppMsg::PdfZoom(1));
        for widget in [
            pdf_previous.upcast_ref::<gtk::Widget>(),
            pdf_page.upcast_ref::<gtk::Widget>(),
            pdf_next.upcast_ref::<gtk::Widget>(),
            zoom_out.upcast_ref::<gtk::Widget>(),
            zoom_fit.upcast_ref::<gtk::Widget>(),
            pdf_zoom.upcast_ref::<gtk::Widget>(),
            zoom_in.upcast_ref::<gtk::Widget>(),
        ] {
            pdf_controls.append(widget);
        }
        hero.append(&pdf_controls);
        info_box.append(&hero);

        let identity = gtk::Box::new(gtk::Orientation::Vertical, 3);
        identity.add_css_class("file-identity");
        let name = gtk::Label::new(Some("Home"));
        name.add_css_class("preview-name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let meta = gtk::Label::new(Some("Folder"));
        meta.add_css_class("preview-meta");
        identity.append(&name);
        identity.append(&meta);
        info_box.append(&identity);

        let details_section = inspector_section("Details");
        let (detail_type_row, detail_type_value) = inspector_row("Kind");
        let (detail_dimensions_row, detail_dimensions_value) = inspector_row("Dimensions");
        details_section.append(&detail_type_row);
        details_section.append(&detail_dimensions_row);
        details_section.set_visible(false);
        info_box.append(&details_section);

        let general_section = inspector_section("General");
        let (path_row, path_value) = inspector_row("Path");
        let (size_row, size_value) = inspector_row("Size");
        let (modified_row, modified_value) = inspector_row("Modified");
        let (created_row, created_value) = inspector_row("Created");
        let (accessed_row, accessed_value) = inspector_row("Accessed");
        let (extension_row, extension_value) = inspector_row("Extension");
        let (hidden_row, hidden_value) = inspector_row("Hidden");
        let (read_only_row, read_only_value) = inspector_row("Read only");
        for row in [
            &path_row,
            &size_row,
            &modified_row,
            &created_row,
            &accessed_row,
            &extension_row,
            &hidden_row,
            &read_only_row,
        ] {
            general_section.append(row);
        }
        info_box.append(&general_section);

        let permissions_section = inspector_section("Permissions");
        let (permission_owner_row, permission_owner_value) = inspector_row("Owner");
        let (permission_group_row, permission_group_value) = inspector_row("Group");
        let (permission_others_row, permission_others_value) = inspector_row("Others");
        let (permission_mode_row, permission_mode_value) = inspector_row("Mode");
        let (permission_octal_row, permission_octal_value) = inspector_row("Octal");
        let (permission_identity_row, permission_identity_value) = inspector_row("Owner / group");
        for row in [
            &permission_owner_row,
            &permission_group_row,
            &permission_others_row,
            &permission_mode_row,
            &permission_octal_row,
            &permission_identity_row,
        ] {
            permissions_section.append(row);
        }
        let permission_edit = gtk::Button::with_label("Edit Permissions…");
        permission_edit.add_css_class("permission-edit-button");
        permission_edit.set_halign(gtk::Align::Start);
        connect_button(&permission_edit, sender, || {
            AppMsg::ExecuteCommand(CommandId::Permissions)
        });
        permissions_section.append(&permission_edit);
        info_box.append(&permissions_section);

        let git_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
        git_section.add_css_class("inspector-section");
        git_section.add_css_class("inspector-section-git");
        let git_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        git_header.add_css_class("inspector-section-header");
        let git_title = gtk::Label::new(Some("Git"));
        git_title.add_css_class("section-title");
        git_title.set_xalign(0.0);
        git_title.set_hexpand(true);
        git_header.append(&git_title);
        let git_state_pill = gtk::Label::new(None);
        git_state_pill.add_css_class("inspector-section-pill");
        git_state_pill.add_css_class("git-pill");
        git_state_pill.set_valign(gtk::Align::Center);
        git_header.append(&git_state_pill);
        git_section.append(&git_header);
        let (git_branch_row, git_branch_value) = inspector_row("Branch");
        let (git_upstream_row, git_upstream_value) = inspector_row("Upstream");
        let (git_item_row, git_item_value) = inspector_row("This item");
        let (git_tree_row, git_tree_value) = inspector_row("Working tree");
        let git_commit_row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        git_commit_row.add_css_class("inspector-row");
        git_commit_row.add_css_class("git-commit-row");
        let git_commit_key = gtk::Label::new(Some("Last commit"));
        git_commit_key.add_css_class("inspector-key");
        git_commit_key.set_xalign(0.0);
        let git_commit_subject = gtk::Label::new(None);
        git_commit_subject.add_css_class("git-commit-subject");
        git_commit_subject.set_xalign(0.0);
        git_commit_subject.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let git_commit_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let git_commit_hash = gtk::Label::new(None);
        git_commit_hash.add_css_class("git-commit-hash");
        git_commit_hash.set_valign(gtk::Align::Center);
        let git_commit_meta = gtk::Label::new(None);
        git_commit_meta.add_css_class("git-commit-meta");
        git_commit_meta.set_xalign(0.0);
        git_commit_meta.set_hexpand(true);
        git_commit_meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
        git_commit_line.append(&git_commit_hash);
        git_commit_line.append(&git_commit_meta);
        git_commit_row.append(&git_commit_key);
        git_commit_row.append(&git_commit_subject);
        git_commit_row.append(&git_commit_line);
        let (git_root_row, git_root_value) = inspector_row("Repository");
        for row in [
            &git_branch_row,
            &git_upstream_row,
            &git_item_row,
            &git_tree_row,
            &git_commit_row,
            &git_root_row,
        ] {
            git_section.append(row);
        }
        git_section.set_visible(false);
        info_box.append(&git_section);

        let info_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&info_box)
            .build();
        stack.add_named(&info_scroll, Some("info"));

        let work_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        work_box.add_css_class("inspector-page");
        let work_header = inspector_page_actions();
        let work_summary = gtk::Label::new(Some("Idle"));
        work_summary.add_css_class("inspector-section-pill");
        work_summary.set_valign(gtk::Align::Center);
        work_header.append(&work_summary);
        work_box.append(&work_header);
        let work_empty = inspector_panel_empty(
            "commander-refresh-cw-symbolic",
            "No file operations are running.",
        );
        work_box.append(&work_empty);
        let work_list = job_view::JobListWidgets::new();
        let work_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(220)
            .child(&work_list.root)
            .build();
        work_box.append(&work_scroll);
        stack.add_named(&work_box, Some("work"));

        let log_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        log_box.add_css_class("inspector-page");
        let log_header = inspector_page_actions();
        let log_clear = gtk::Button::with_label("Clear");
        log_clear.add_css_class("inspector-section-action");
        log_clear.set_valign(gtk::Align::Center);
        connect_button(&log_clear, sender, || AppMsg::ClearOperationLog);
        log_header.append(&log_clear);
        log_box.append(&log_header);
        let log_empty =
            inspector_panel_empty("commander-file-symbolic", "No operation log entries.");
        log_box.append(&log_empty);
        let log_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        log_list.add_css_class("inspector-log-list");
        let log_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&log_list)
            .build();
        log_box.append(&log_scroll);
        stack.add_named(&log_box, Some("log"));
        panel.append(&stack);
        root.set_child(Some(&panel));

        Self {
            root,
            stack,
            tab_title,
            tabs: [info_tab, work_tab, log_tab],
            content_stack,
            icon,
            picture,
            pdf_picture,
            video,
            text_view,
            content_status,
            pdf_controls,
            pdf_previous,
            pdf_next,
            pdf_page,
            pdf_zoom,
            rendered_preview: Cell::new((u64::MAX, false, usize::MAX)),
            identity,
            name,
            meta,
            details_section,
            detail_type_value,
            detail_dimensions_value,
            path_value,
            size_value,
            modified_value,
            created_value,
            accessed_value,
            extension_value,
            hidden_value,
            read_only_value,
            general_section,
            permissions_section,
            permission_owner_value,
            permission_group_value,
            permission_others_value,
            permission_mode_value,
            permission_octal_value,
            permission_identity_value,
            git_section,
            git_state_pill,
            git_branch_value,
            git_upstream_value,
            git_item_value,
            git_tree_value,
            git_commit_row,
            git_commit_subject,
            git_commit_hash,
            git_commit_meta,
            git_root_value,
            work_summary,
            work_empty,
            work_scroll,
            work_list,
            log_list,
            log_controls: log_header,
            log_empty,
            log_scroll,
            log_clear,
            rendered_log_revision: Cell::new(u64::MAX),
        }
    }

    pub(super) fn focus_page(&self, page: InspectorPage) {
        self.tabs[match page {
            InspectorPage::Info => 0,
            InspectorPage::Work => 1,
            InspectorPage::Log => 2,
        }]
        .grab_focus();
    }

    pub(super) fn render(&self, model: &AppModel, sender: &ComponentSender<AppModel>) {
        self.tab_title.set_label(model.inspector_page.title());
        self.stack
            .set_visible_child_name(model.inspector_page.name());
        for (button, page) in
            self.tabs
                .iter()
                .zip([InspectorPage::Info, InspectorPage::Work, InspectorPage::Log])
        {
            button.set_active(model.inspector_page == page);
        }
        let selected_count = model.pane(model.active_pane).selection.len();
        self.render_content(&model.preview_state, selected_count);
        self.render_operation_list(model, sender);
        self.render_log(model);
        if selected_count > 1 {
            self.render_selection_summary(model, selected_count);
        } else if let Some(preview) = model.preview_state.content.as_ref() {
            self.render_preview_metadata(preview, model);
        } else if model.preview_state.loading {
            self.identity.set_visible(true);
            self.name.set_label(
                model
                    .preview_state
                    .path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(OsStr::to_str)
                    .unwrap_or("Loading…"),
            );
            self.meta.set_label("Loading metadata…");
            self.set_sections_visible(false, false, false, false);
        } else {
            self.identity.set_visible(false);
            self.set_sections_visible(false, false, false, false);
        }
    }

    pub(super) fn render_selection_summary(&self, model: &AppModel, selected_count: usize) {
        self.identity.set_visible(true);
        self.name
            .set_label(&format!("{selected_count} items selected"));
        self.set_sections_visible(true, false, false, false);
        let Some(summary) = model.inspector_selection.summary.as_ref() else {
            self.meta.set_label("Calculating selection…");
            self.path_value.set_label("—");
            self.size_value.set_label("Calculating…");
            self.modified_value.set_label("—");
            self.clear_general_metadata();
            return;
        };
        self.name
            .set_label(&format!("{} items selected", summary.count));
        let mut types = Vec::new();
        if summary.files > 0 {
            types.push(format!("{} file{}", summary.files, plural(summary.files)));
        }
        if summary.folders > 0 {
            types.push(format!(
                "{} folder{}",
                summary.folders,
                plural(summary.folders)
            ));
        }
        if summary.other > 0 {
            types.push(format!("{} other", summary.other));
        }
        self.meta.set_label(&types.join(" · "));
        self.path_value.set_label(
            &summary
                .location
                .as_ref()
                .map_or_else(|| "Multiple locations".to_owned(), ToString::to_string),
        );
        let folder_bytes = model
            .folder_measure
            .result
            .as_ref()
            .map_or(0, |result| result.bytes);
        let total = summary.known_bytes.saturating_add(folder_bytes);
        if model.folder_measure.loading {
            let label = if total > 0 {
                format!("{} + calculating…", format_size(total, EntryKind::File))
            } else {
                "Calculating…".to_owned()
            };
            self.size_value.set_label(&label);
        } else {
            let skipped = summary.unknown_sizes
                + model
                    .folder_measure
                    .result
                    .as_ref()
                    .map_or(0, |result| result.skipped as usize);
            self.size_value.set_label(&if skipped == 0 {
                format_size(total, EntryKind::File)
            } else {
                format!(
                    "{} · {} not counted",
                    format_size(total, EntryKind::File),
                    skipped
                )
            });
        }
        self.modified_value.set_label("—");
        self.clear_general_metadata();
    }

    pub(super) fn render_preview_metadata(&self, preview: &Preview, model: &AppModel) {
        self.identity.set_visible(true);
        self.set_sections_visible(true, true, true, true);
        self.name.set_label(
            preview
                .path
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("Item"),
        );
        let type_label = preview_type_label(preview);
        self.meta.set_label(&type_label);
        let parent = preview
            .path
            .parent()
            .map_or_else(|| preview.path.to_string(), |path| path.to_string());
        self.path_value.set_label(&parent);
        self.path_value
            .set_tooltip_text(Some(&preview.path.to_string()));
        self.size_value
            .set_label(&if preview.metadata.kind == EntryKind::Directory {
                if model.folder_measure.loading {
                    "Calculating…".to_owned()
                } else if let Some(result) = model.folder_measure.result.as_ref() {
                    let suffix = if result.skipped == 0 {
                        String::new()
                    } else {
                        format!(" · {} skipped", result.skipped)
                    };
                    format!(
                        "{} · {} items{suffix}",
                        format_size(result.bytes, EntryKind::File),
                        result.items
                    )
                } else {
                    "—".to_owned()
                }
            } else {
                format_size(preview.metadata.size, preview.metadata.kind)
            });
        self.modified_value.set_label(
            &preview
                .metadata
                .modified
                .map_or_else(|| "—".to_owned(), |value| format_timestamp(value.seconds)),
        );
        self.created_value.set_label(
            &preview
                .metadata
                .created
                .map_or_else(|| "—".to_owned(), |value| format_timestamp(value.seconds)),
        );
        self.accessed_value.set_label(
            &preview
                .metadata
                .accessed
                .map_or_else(|| "—".to_owned(), |value| format_timestamp(value.seconds)),
        );
        self.extension_value.set_label(
            preview
                .path
                .as_path()
                .extension()
                .and_then(OsStr::to_str)
                .filter(|extension| !extension.is_empty())
                .unwrap_or("—"),
        );
        let hidden = preview
            .path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."));
        let read_only = preview.metadata.mode.is_some_and(|mode| mode & 0o222 == 0);
        self.hidden_value.set_label(if hidden { "●" } else { "○" });
        self.read_only_value
            .set_label(if read_only { "●" } else { "○" });
        if let Some(mode) = preview.metadata.mode {
            self.permissions_section.set_visible(true);
            self.permission_owner_value
                .set_label(&permission_triplet(mode, 6));
            self.permission_group_value
                .set_label(&permission_triplet(mode, 3));
            self.permission_others_value
                .set_label(&permission_triplet(mode, 0));
            self.permission_mode_value.set_label(&format_mode(mode));
            self.permission_octal_value
                .set_label(&format!("{:04o}", mode & 0o7777));
        } else {
            self.permissions_section.set_visible(false);
        }
        self.permission_identity_value.set_label(&match (
            preview.metadata.owner,
            preview.metadata.group,
        ) {
            (Some(owner), Some(group)) => format!("{owner} / {group}"),
            (Some(owner), None) => owner.to_string(),
            (None, Some(group)) => group.to_string(),
            (None, None) => "—".to_owned(),
        });
        self.render_preview_details(preview);
        if let Some(info) = model.inspector_git.info.as_ref() {
            self.git_section.set_visible(true);
            self.render_git(info);
        } else {
            self.git_section.set_visible(false);
        }
    }

    pub(super) fn render_preview_details(&self, preview: &Preview) {
        let details = match &preview.payload {
            PreviewPayload::Image { width, height, .. } => {
                Some(("Image".to_owned(), format!("{width} × {height}")))
            }
            PreviewPayload::Pdf {
                page_count,
                page_number,
                ..
            } => Some((
                "PDF document".to_owned(),
                format!("Page {page_number} of {page_count}"),
            )),
            PreviewPayload::Text {
                language,
                truncated,
                ..
            } => Some((
                format!("{language} text"),
                if *truncated {
                    "Preview truncated".to_owned()
                } else {
                    "Complete preview".to_owned()
                },
            )),
            PreviewPayload::Media { kind, .. } => {
                Some(((*kind).to_owned(), "Native media controls".to_owned()))
            }
            _ => None,
        };
        self.details_section.set_visible(details.is_some());
        if let Some((kind, detail)) = details {
            self.detail_type_value.set_label(&kind);
            self.detail_dimensions_value.set_label(&detail);
        }
    }

    pub(super) fn clear_general_metadata(&self) {
        self.created_value.set_label("—");
        self.accessed_value.set_label("—");
        self.extension_value.set_label("—");
        self.hidden_value.set_label("—");
        self.read_only_value.set_label("—");
    }

    fn render_git(&self, info: &GitInfo) {
        let (pill_text, pill_class) = match info.item_state {
            GitItemState::Clean => ("Clean", "git-pill-clean"),
            GitItemState::Modified => ("Modified", "git-pill-modified"),
            GitItemState::Staged => ("Staged", "git-pill-staged"),
            GitItemState::Untracked => ("Untracked", "git-pill-untracked"),
            GitItemState::Ignored => ("Ignored", "git-pill-ignored"),
            GitItemState::Conflict => ("Conflict", "git-pill-conflict"),
        };
        self.git_state_pill.set_label(pill_text);
        for class in [
            "git-pill-clean",
            "git-pill-modified",
            "git-pill-staged",
            "git-pill-untracked",
            "git-pill-ignored",
            "git-pill-conflict",
        ] {
            self.git_state_pill.remove_css_class(class);
        }
        self.git_state_pill.add_css_class(pill_class);
        self.git_branch_value.set_label(&info.branch);
        self.git_branch_value.set_tooltip_text(Some(&info.branch));
        let upstream = match (&info.upstream, info.ahead, info.behind) {
            (None, _, _) => "Not set".to_owned(),
            (Some(_), 0, 0) => "Up to date".to_owned(),
            (Some(_), ahead, behind) => {
                let mut parts = Vec::new();
                if ahead > 0 {
                    parts.push(format!("{ahead} ahead"));
                }
                if behind > 0 {
                    parts.push(format!("{behind} behind"));
                }
                parts.join(", ")
            }
        };
        self.git_upstream_value.set_label(&upstream);
        self.git_upstream_value
            .set_tooltip_text(info.upstream.as_deref());
        self.git_item_value.set_label(&info.item_summary);
        self.git_tree_value.set_label(&info.tree_summary);
        if let Some(commit) = &info.last_commit {
            self.git_commit_row.set_visible(true);
            self.git_commit_subject.set_label(&commit.subject);
            self.git_commit_subject
                .set_tooltip_text(Some(&commit.subject));
            self.git_commit_hash.set_label(&commit.hash);
            self.git_commit_meta
                .set_label(&format!("{} · {}", commit.relative_time, commit.author));
        } else {
            self.git_commit_row.set_visible(false);
        }
        let root_name = std::path::Path::new(&info.root)
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or(info.root.as_str());
        self.git_root_value.set_label(root_name);
        self.git_root_value.set_tooltip_text(Some(&info.root));
    }

    pub(super) fn set_sections_visible(
        &self,
        general: bool,
        permissions: bool,
        details: bool,
        git: bool,
    ) {
        self.general_section.set_visible(general);
        self.permissions_section.set_visible(permissions);
        self.details_section.set_visible(details);
        self.git_section.set_visible(git);
    }

    pub(super) fn render_content(&self, state: &PreviewState, selected_count: usize) {
        let key = (state.generation, state.loading, selected_count);
        if self.rendered_preview.get() == key {
            return;
        }
        self.rendered_preview.set(key);
        self.pdf_controls.set_visible(false);
        if selected_count > 1 {
            pause_video(&self.video);
            self.content_stack.set_visible_child_name("multi");
            return;
        }
        if state.loading {
            self.content_stack.set_visible_child_name("loading");
            return;
        }
        if let Some(error) = &state.error {
            self.content_status.set_label(error);
            self.content_stack.set_visible_child_name("status");
            return;
        }
        let Some(preview) = &state.content else {
            self.content_stack.set_visible_child_name("empty");
            return;
        };
        match &preview.payload {
            PreviewPayload::Directory => {
                pause_video(&self.video);
                crate::icons::set_file_icon(
                    &self.icon,
                    preview.metadata.kind,
                    preview.path.file_name().unwrap_or_default(),
                );
                self.content_stack.set_visible_child_name("icon");
            }
            PreviewPayload::Image {
                rgba,
                width,
                height,
            } => {
                pause_video(&self.video);
                let Ok(width_i32) = i32::try_from(*width) else {
                    self.content_status
                        .set_label("Image is too wide to preview");
                    self.content_stack.set_visible_child_name("status");
                    return;
                };
                let Ok(height_i32) = i32::try_from(*height) else {
                    self.content_status
                        .set_label("Image is too tall to preview");
                    self.content_stack.set_visible_child_name("status");
                    return;
                };
                let bytes = glib::Bytes::from_owned(rgba.clone());
                let texture = gdk::MemoryTexture::new(
                    width_i32,
                    height_i32,
                    gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    usize::try_from(*width).unwrap_or(0).saturating_mul(4),
                );
                self.picture.set_paintable(Some(&texture));
                self.content_stack.set_visible_child_name("image");
            }
            PreviewPayload::Pdf {
                rgba,
                width,
                height,
                page_count,
                page_number,
                scale,
                ..
            } => {
                pause_video(&self.video);
                if set_picture_rgba(&self.pdf_picture, rgba, *width, *height) {
                    let (Ok(width_request), Ok(height_request)) =
                        (i32::try_from(*width), i32::try_from(*height))
                    else {
                        self.content_status
                            .set_label("PDF page is too large to preview");
                        self.content_stack.set_visible_child_name("status");
                        return;
                    };
                    self.pdf_picture
                        .set_size_request(width_request, height_request);
                    self.pdf_picture.set_tooltip_text(Some(&format!(
                        "PDF · page {page_number} of {page_count}"
                    )));
                    self.content_stack.set_visible_child_name("pdf");
                    self.pdf_page
                        .set_label(&format!("{page_number} / {page_count}"));
                    self.pdf_zoom
                        .set_label(&format!("{}%", (*scale * 100.0).round() as u32));
                    self.pdf_previous.set_sensitive(*page_number > 1);
                    self.pdf_next.set_sensitive(*page_number < *page_count);
                    self.pdf_controls.set_visible(true);
                } else {
                    self.content_status
                        .set_label("PDF page is too large to preview");
                    self.content_stack.set_visible_child_name("status");
                }
            }
            PreviewPayload::Text {
                content,
                language,
                truncated,
                highlights,
            } => {
                pause_video(&self.video);
                text::render(&self.text_view, content, highlights);
                self.text_view.set_tooltip_text(Some(&format!(
                    "{language}{}",
                    if *truncated {
                        " · preview truncated"
                    } else {
                        ""
                    }
                )));
                self.content_stack.set_visible_child_name("text");
            }
            PreviewPayload::Media { bytes, kind } => {
                set_video_bytes(&self.video, bytes);
                self.video.set_tooltip_text(Some(kind));
                self.content_stack.set_visible_child_name("media");
            }
            PreviewPayload::Unsupported => {
                pause_video(&self.video);
                crate::icons::set_file_icon(
                    &self.icon,
                    preview.metadata.kind,
                    preview.path.file_name().unwrap_or_default(),
                );
                self.content_stack.set_visible_child_name("icon");
            }
        }
    }

    pub(super) fn render_log(&self, model: &AppModel) {
        if self.rendered_log_revision.get() == model.operation_log_revision {
            return;
        }
        self.rendered_log_revision.set(model.operation_log_revision);
        while let Some(child) = self.log_list.first_child() {
            self.log_list.remove(&child);
        }
        let empty = model.operation_log.is_empty();
        self.log_empty.set_visible(empty);
        self.log_controls.set_visible(!empty);
        self.log_list.set_visible(!empty);
        self.log_scroll.set_visible(!empty);
        self.log_clear.set_visible(!empty);
        for entry in model.operation_log.iter().rev() {
            let card = gtk::Box::new(gtk::Orientation::Horizontal, 9);
            card.add_css_class("inspector-log-entry");
            card.add_css_class(match entry.status {
                OperationLogStatus::Info => "log-info",
                OperationLogStatus::Success => "log-success",
                OperationLogStatus::Warning => "log-warning",
                OperationLogStatus::Error => "log-error",
            });
            let dot = gtk::Label::new(Some("●"));
            dot.add_css_class("inspector-log-dot");
            let body = gtk::Box::new(gtk::Orientation::Vertical, 3);
            body.set_hexpand(true);
            let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let title = gtk::Label::new(Some(&entry.label));
            title.set_xalign(0.0);
            title.set_hexpand(true);
            title.add_css_class("inspector-log-title");
            let time = gtk::Label::new(Some(&relative_log_time(entry.created_at)));
            time.add_css_class("inspector-log-time");
            header.append(&title);
            header.append(&time);
            let detail = gtk::Label::new(Some(&entry.detail));
            detail.set_xalign(0.0);
            detail.set_wrap(true);
            detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            detail.set_selectable(true);
            detail.add_css_class("inspector-log-detail");
            detail.set_tooltip_text(Some(&format!("Event {} · {}", entry.id, entry.detail)));
            body.append(&header);
            body.append(&detail);
            card.append(&dot);
            card.append(&body);
            self.log_list.append(&card);
        }
    }

    pub(super) fn render_operation_list(
        &self,
        model: &AppModel,
        sender: &ComponentSender<AppModel>,
    ) {
        let active_count = model
            .operations
            .values()
            .filter(|operation| operation.is_active())
            .count();
        self.work_summary
            .set_label(&if model.operations.is_empty() {
                "Idle".to_owned()
            } else if active_count > 0 {
                format!("{active_count} active")
            } else {
                "Recent".to_owned()
            });
        self.work_empty.set_visible(model.operations.is_empty());
        self.work_scroll.set_visible(!model.operations.is_empty());
        self.work_list
            .render(&model.operations, sender.input_sender());
    }
}

pub(super) fn inspector_row(name: &str) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("inspector-row");
    let key = gtk::Label::new(Some(name));
    key.add_css_class("inspector-key");
    key.set_xalign(0.0);
    let value = gtk::Label::new(None);
    value.add_css_class("inspector-value");
    value.set_xalign(1.0);
    value.set_hexpand(true);
    value.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    row.append(&key);
    row.append(&value);
    (row, value)
}

pub(super) fn inspector_section(title: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    section.add_css_class("inspector-section");
    let title = gtk::Label::new(Some(title));
    title.add_css_class("section-title");
    title.set_xalign(0.0);
    section.append(&title);
    section
}

pub(super) fn inspector_page_actions() -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("inspector-page-actions");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&spacer);
    header
}

pub(super) fn inspector_panel_empty(icon_name: &str, message: &str) -> gtk::Box {
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
    empty.add_css_class("inspector-panel-empty");
    empty.set_halign(gtk::Align::Fill);
    empty.set_valign(gtk::Align::Fill);
    empty.set_vexpand(true);
    let top_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    top_spacer.set_vexpand(true);
    let bottom_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bottom_spacer.set_vexpand(true);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(28);
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    empty.append(&top_spacer);
    empty.append(&icon);
    empty.append(&label);
    empty.append(&bottom_spacer);
    empty
}

pub(super) fn measure_folder_paths(
    vfs: &dyn Vfs,
    paths: &[VPath],
    cancel: &CancelToken,
) -> Result<FolderMeasureResult, String> {
    let mut result = FolderMeasureResult::default();
    let mut stack = paths.to_vec();
    let mut visited = HashSet::new();
    while let Some(path) = stack.pop() {
        cancel
            .check()
            .map_err(|_| "Folder size calculation cancelled".to_owned())?;
        let Ok(metadata) = vfs.stat(&path, false) else {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        };
        if metadata.kind != EntryKind::Directory {
            result.bytes = result.bytes.saturating_add(metadata.size);
            result.items = result.items.saturating_add(1);
            continue;
        }
        // Avoid directory cycles while summing logical file sizes: two hard links
        // are two selected entries, and each contributes its displayed size.
        if let Some(identity) = metadata.identity
            && !visited.insert(identity)
        {
            continue;
        }
        let Ok(entries) = vfs.read_dir(&path, cancel) else {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        };
        for entry in entries {
            cancel
                .check()
                .map_err(|_| "Folder size calculation cancelled".to_owned())?;
            match entry {
                Ok(entry) => {
                    let child = path.join_name(entry.name());
                    if entry.kind() == EntryKind::Directory {
                        result.items = result.items.saturating_add(1);
                    }
                    // stat(false) measures links themselves without following them.
                    stack.push(child);
                }
                Err(_) => result.skipped = result.skipped.saturating_add(1),
            }
        }
    }
    Ok(result)
}

pub(super) fn git_info_for_path(path: &VPath) -> Option<GitInfo> {
    let native = path.as_path();
    let directory = if native.is_dir() {
        native
    } else {
        native.parent()?
    };
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let root = git(&["rev-parse", "--show-toplevel"])?.trim().to_owned();
    let branch = git(&["branch", "--show-current"])
        .map(|branch| branch.trim().to_owned())
        .filter(|branch| !branch.is_empty())
        .or_else(|| {
            git(&["rev-parse", "--short", "HEAD"])
                .map(|hash| format!("Detached at {}", hash.trim()))
        })
        .unwrap_or_else(|| "No commits yet".to_owned());
    let upstream = git(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let (ahead, behind) = upstream
        .as_ref()
        .and_then(|_| git(&["rev-list", "--left-right", "--count", "HEAD...@{u}"]))
        .and_then(|counts| {
            let mut parts = counts.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        })
        .unwrap_or((0, 0));
    let item_path = native.to_string_lossy().into_owned();
    let item_entries = git(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
        &item_path,
    ])
    .map(|output| parse_porcelain(&output))
    .unwrap_or_default();
    let ignored = item_entries.is_empty()
        && Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["check-ignore", "-q", "--", &item_path])
            .status()
            .is_ok_and(|status| status.success());
    let (item_state, item_summary) = if ignored {
        (GitItemState::Ignored, "Ignored by .gitignore".to_owned())
    } else if native.is_dir() {
        summarize_entries(&item_entries)
    } else {
        describe_file(item_entries.first().map(|(code, _)| code.as_str()))
    };
    let tree_entries = git(&["status", "--porcelain=v1", "-z"])
        .map(|output| parse_porcelain(&output))
        .unwrap_or_default();
    let tree_summary = summarize_entries(&tree_entries).1;
    let last_commit = git(&[
        "log",
        "-1",
        "--format=%h%x1f%s%x1f%ar%x1f%an",
        "--",
        &item_path,
    ])
    .and_then(|line| {
        let mut fields = line.trim_end().split('\u{1f}');
        Some(GitCommit {
            hash: fields.next()?.to_owned(),
            subject: fields.next()?.to_owned(),
            relative_time: fields.next()?.to_owned(),
            author: fields.next()?.to_owned(),
        })
    })
    .filter(|commit| !commit.hash.is_empty());
    Some(GitInfo {
        branch,
        upstream,
        ahead,
        behind,
        item_state,
        item_summary,
        tree_summary,
        last_commit,
        root,
    })
}

/// Splits `git status --porcelain=v1 -z` output into `(XY, path)` pairs,
/// skipping the extra source-path record that follows a rename.
pub(super) fn parse_porcelain(output: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut records = output.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let code = record[..2].to_owned();
        let path = record[3..].to_owned();
        if code.starts_with('R') || code.starts_with('C') {
            records.next();
        }
        entries.push((code, path));
    }
    entries
}

fn is_conflict(code: &str) -> bool {
    matches!(code, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU")
}

/// Describes a single file from its porcelain status code.
pub(super) fn describe_file(code: Option<&str>) -> (GitItemState, String) {
    let Some(code) = code else {
        return (GitItemState::Clean, "Clean".to_owned());
    };
    if is_conflict(code) {
        return (GitItemState::Conflict, "Merge conflict".to_owned());
    }
    let (index, worktree) = (&code[..1], &code[1..]);
    match (index, worktree) {
        ("?", "?") => (GitItemState::Untracked, "Untracked".to_owned()),
        ("!", "!") => (GitItemState::Ignored, "Ignored by .gitignore".to_owned()),
        (_, "D") | ("D", _) => (GitItemState::Modified, "Deleted".to_owned()),
        ("A", " ") => (GitItemState::Staged, "Added, staged".to_owned()),
        ("A", _) => (
            GitItemState::Modified,
            "Added, with unstaged changes".to_owned(),
        ),
        ("R", " ") => (GitItemState::Staged, "Renamed, staged".to_owned()),
        ("R", _) => (
            GitItemState::Modified,
            "Renamed, with unstaged changes".to_owned(),
        ),
        (" ", _) => (GitItemState::Modified, "Modified, not staged".to_owned()),
        (_, " ") => (GitItemState::Staged, "Modified, staged".to_owned()),
        _ => (GitItemState::Modified, "Modified, partly staged".to_owned()),
    }
}

/// Aggregates porcelain entries for a folder or the whole tree.
pub(super) fn summarize_entries(entries: &[(String, String)]) -> (GitItemState, String) {
    let (mut staged, mut modified, mut untracked, mut conflicts) = (0, 0, 0, 0);
    for (code, _) in entries {
        if is_conflict(code) {
            conflicts += 1;
        } else if code == "??" {
            untracked += 1;
        } else if code == "!!" {
        } else {
            if !code.starts_with(' ') {
                staged += 1;
            }
            if !code.ends_with(' ') {
                modified += 1;
            }
        }
    }
    let mut parts = Vec::new();
    let plural = |count: usize, word: &str| format!("{count} {word}");
    if conflicts > 0 {
        parts.push(plural(
            conflicts,
            if conflicts == 1 {
                "conflict"
            } else {
                "conflicts"
            },
        ));
    }
    if staged > 0 {
        parts.push(plural(staged, "staged"));
    }
    if modified > 0 {
        parts.push(plural(modified, "modified"));
    }
    if untracked > 0 {
        parts.push(plural(untracked, "untracked"));
    }
    if parts.is_empty() {
        return (GitItemState::Clean, "Clean".to_owned());
    }
    let state = if conflicts > 0 {
        GitItemState::Conflict
    } else if modified > 0 {
        GitItemState::Modified
    } else if staged > 0 {
        GitItemState::Staged
    } else {
        GitItemState::Untracked
    };
    (state, parts.join(", "))
}

pub(super) fn set_video_bytes(video: &gtk::Video, data: &[u8]) {
    let bytes = glib::Bytes::from_owned(data.to_vec());
    let stream = gio::MemoryInputStream::from_bytes(&bytes);
    let media = gtk::MediaFile::for_input_stream(&stream);
    video.set_media_stream(Some(&media));
    media.play();
}

pub(super) fn set_picture_rgba(
    picture: &gtk::Picture,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> bool {
    let (Ok(width), Ok(height)) = (i32::try_from(width), i32::try_from(height)) else {
        return false;
    };
    let bytes = glib::Bytes::from_owned(rgba.to_vec());
    let texture = gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        usize::try_from(width).unwrap_or(0).saturating_mul(4),
    );
    picture.set_paintable(Some(&texture));
    true
}

pub(super) fn fitted_pdf_scale(
    rendered_width: u32,
    rendered_height: u32,
    current_scale: f32,
    inspector_width: i32,
) -> f32 {
    if rendered_width == 0
        || rendered_height == 0
        || !current_scale.is_finite()
        || current_scale <= 0.0
    {
        return PDF_MIN_SCALE;
    }
    let native_width = rendered_width as f32 / current_scale;
    let native_height = rendered_height as f32 / current_scale;
    let available_width = inspector_width
        .saturating_sub(PDF_PREVIEW_HORIZONTAL_INSET)
        .max(1) as f32;
    (available_width / native_width)
        .min(PDF_VIEWPORT_HEIGHT / native_height)
        .clamp(PDF_MIN_SCALE, PDF_MAX_SCALE)
}

pub(super) fn pause_video(video: &gtk::Video) {
    if let Some(stream) = video.media_stream() {
        stream.pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_fit_uses_both_viewport_dimensions() {
        let portrait = fitted_pdf_scale(298, 421, 0.5, 340);
        assert!((portrait - 0.2494).abs() < 0.002);

        let landscape = fitted_pdf_scale(421, 298, 0.5, 340);
        assert!((landscape - 0.3325).abs() < 0.002);
        assert_eq!(fitted_pdf_scale(0, 0, 0.0, 340), PDF_MIN_SCALE);
    }

    #[test]
    fn inspector_folder_measurement_counts_nested_content() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir(temporary.path().join("nested")).expect("nested directory");
        std::fs::write(temporary.path().join("one"), b"1234").expect("first file");
        std::fs::write(temporary.path().join("nested/two"), b"123456").expect("second file");

        let result = measure_folder_paths(
            &LocalFs,
            &[VPath::from(temporary.path())],
            &CancelToken::new(),
        )
        .expect("folder measurement");

        assert_eq!(result.bytes, 10);
        assert_eq!(result.items, 3);
        assert_eq!(result.skipped, 0);
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;

    #[test]
    fn porcelain_records_are_split_and_renames_skip_their_source() {
        let output = " M README.md\0?? new.txt\0R  old.rs\0new.rs\0MM both.rs\0";
        let entries = parse_porcelain(output);
        let codes: Vec<&str> = entries.iter().map(|(code, _)| code.as_str()).collect();
        assert_eq!(codes, vec![" M", "??", "R ", "MM"]);
        assert_eq!(entries[2].1, "old.rs");
    }

    #[test]
    fn file_states_read_naturally() {
        assert_eq!(describe_file(None).0, GitItemState::Clean);
        assert_eq!(describe_file(Some("??")).0, GitItemState::Untracked);
        assert_eq!(describe_file(Some(" M")).1, "Modified, not staged");
        assert_eq!(describe_file(Some("M ")).0, GitItemState::Staged);
        assert_eq!(describe_file(Some("UU")).0, GitItemState::Conflict);
    }

    #[test]
    fn folder_summary_counts_each_kind() {
        let entries = vec![
            (" M".to_owned(), "a".to_owned()),
            ("MM".to_owned(), "b".to_owned()),
            ("A ".to_owned(), "c".to_owned()),
            ("??".to_owned(), "d".to_owned()),
        ];
        let (state, summary) = summarize_entries(&entries);
        assert_eq!(state, GitItemState::Modified);
        assert_eq!(summary, "2 staged, 2 modified, 1 untracked");
        assert_eq!(summarize_entries(&[]).1, "Clean");
    }
}

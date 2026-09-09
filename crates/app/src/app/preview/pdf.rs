//! Shared continuous PDF reader. Only pages near the viewport have GTK widgets.

use super::super::*;
use dualpane_thumbs::pdf::{
    MAX_SCALE, MIN_SCALE, PageResults, PageSize, PdfDocument, RequestError,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[cfg(test)]
mod tests;

const GAP: f64 = 12.0;
const CACHE_BYTES: usize = 48 * 1024 * 1024;

pub(super) struct PdfView {
    pub root: gtk::Box,
    inner: Rc<View>,
}

struct View {
    root: gtk::Box,
    toolbar: gtk::Box,
    navigation: gtk::Box,
    zoom_bar: gtk::Box,
    scroll: gtk::ScrolledWindow,
    canvas: gtk::Fixed,
    page: gtk::Entry,
    count: gtk::Label,
    previous: gtk::Button,
    next: gtk::Button,
    zoom: gtk::Label,
    fit: gtk::ToggleButton,
    bookmarks: gtk::MenuButton,
    outline: gtk::Box,
    document: RefCell<Option<DocumentView>>,
    scheduled: Cell<bool>,
}

struct DocumentView {
    source: PdfDocument,
    scale: f32,
    pixel_ratio: i32,
    fit_width: bool,
    viewport_width: i32,
    layout: Layout,
    current: usize,
    jump: Option<usize>,
    slots: HashMap<usize, Slot>,
    cache: VecDeque<CachedPage>,
    cache_bytes: usize,
    failed: HashSet<usize>,
    pending: HashSet<usize>,
    results: Option<PageResults>,
    cancel: CancelToken,
}

struct Slot {
    root: gtk::Overlay,
    picture: PageImage,
    label: gtk::Label,
    ready: bool,
}

struct CachedPage {
    index: usize,
    texture: gdk::MemoryTexture,
    bytes: usize,
}

#[derive(Default)]
struct Layout {
    tops: Vec<f64>,
    widths: Vec<i32>,
    heights: Vec<i32>,
    width: i32,
    height: i32,
}

impl Layout {
    fn new(pages: &[PageSize], scale: f32, viewport_width: i32) -> Self {
        let mut layout = Self {
            width: viewport_width.max(1),
            ..Self::default()
        };
        let mut top = GAP;
        for page in pages {
            // Avoid a one-pixel overflow from floating-point fit-width rounding.
            let width = (page.width * scale).round().max(1.0) as i32;
            let height = (page.height * scale).ceil().max(1.0) as i32;
            layout.tops.push(top);
            layout.widths.push(width);
            layout.heights.push(height);
            layout.width = layout.width.max(width + 2 * GAP as i32);
            top += f64::from(height) + GAP;
        }
        layout.height = top.ceil() as i32;
        layout
    }

    fn page_at(&self, y: f64) -> usize {
        self.tops.partition_point(|top| *top <= y).saturating_sub(1)
    }

    fn nearby(&self, top: f64, height: f64) -> std::ops::Range<usize> {
        let start = self.page_at((top - height * 0.5).max(0.0));
        let end = (self.page_at(top + height * 1.5) + 1).min(self.tops.len());
        start..end
    }
}

impl PdfView {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("pdf-preview");
        root.set_hexpand(true);
        let toolbar = gtk::Box::new(gtk::Orientation::Vertical, 4);
        toolbar.add_css_class("pdf-preview-controls");
        toolbar.set_halign(gtk::Align::Fill);
        toolbar.set_hexpand(true);
        let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        navigation.set_halign(gtk::Align::Center);
        navigation.set_hexpand(true);
        let previous = button("commander-chevron-left-symbolic", "Previous page (Alt+↑)");
        let next = button("commander-chevron-right-symbolic", "Next page (Alt+↓)");
        let page = gtk::Entry::new();
        page.add_css_class("pdf-page-entry");
        page.set_width_chars(3);
        page.set_max_width_chars(5);
        page.set_max_length(5);
        page.set_input_purpose(gtk::InputPurpose::Digits);
        gtk::prelude::EntryExt::set_alignment(&page, 0.5);
        page.set_tooltip_text(Some("Go to page (Ctrl+L)"));
        page.update_property(&[gtk::accessible::Property::Label("PDF page number")]);
        let count = gtk::Label::new(None);
        count.add_css_class("pdf-preview-page");
        let bookmarks = gtk::MenuButton::new();
        bookmarks.set_icon_name("commander-list-symbolic");
        bookmarks.add_css_class("pdf-preview-button");
        bookmarks.set_tooltip_text(Some("Document bookmarks"));
        bookmarks.update_property(&[gtk::accessible::Property::Label("Document bookmarks")]);
        let outline = gtk::Box::new(gtk::Orientation::Vertical, 2);
        outline.add_css_class("pdf-bookmarks");
        let outline_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(220)
            .max_content_height(360)
            .propagate_natural_height(true)
            .child(&outline)
            .build();
        let popover = gtk::Popover::new();
        popover.set_child(Some(&outline_scroll));
        bookmarks.set_popover(Some(&popover));
        let zoom_out = button("commander-zoom-out-symbolic", "Zoom out (Ctrl+−)");
        let zoom_in = button("commander-zoom-in-symbolic", "Zoom in (Ctrl++)");
        let fit = gtk::ToggleButton::new();
        fit.set_icon_name("commander-scan-symbolic");
        fit.add_css_class("pdf-preview-button");
        fit.set_tooltip_text(Some("Fit width (Ctrl+0)"));
        fit.update_property(&[gtk::accessible::Property::Label("Fit PDF to width")]);
        let zoom = gtk::Label::new(None);
        zoom.add_css_class("pdf-preview-page");
        for widget in [
            bookmarks.upcast_ref::<gtk::Widget>(),
            previous.upcast_ref(),
            page.upcast_ref(),
            count.upcast_ref(),
            next.upcast_ref(),
        ] {
            navigation.append(widget);
        }
        let zoom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        zoom_bar.set_halign(gtk::Align::Center);
        for widget in [
            zoom_out.upcast_ref::<gtk::Widget>(),
            zoom.upcast_ref(),
            zoom_in.upcast_ref(),
            fit.upcast_ref(),
        ] {
            zoom_bar.append(widget);
        }
        toolbar.append(&navigation);
        toolbar.append(&zoom_bar);
        let canvas = gtk::Fixed::new();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&canvas)
            .build();
        scroll.add_css_class("pdf-preview-scroll");
        scroll.set_focusable(true);
        scroll.update_property(&[gtk::accessible::Property::Label(
            "PDF document. Scroll to read pages; Control L to go to a page.",
        )]);
        root.append(&scroll);
        root.append(&toolbar);
        let inner = Rc::new(View {
            root: root.clone(),
            toolbar,
            navigation,
            zoom_bar,
            scroll,
            canvas,
            page,
            count,
            previous,
            next,
            zoom,
            fit,
            bookmarks,
            outline,
            document: RefCell::new(None),
            scheduled: Cell::new(false),
        });
        for (button, delta) in [(&inner.previous, -1), (&inner.next, 1)] {
            let weak = Rc::downgrade(&inner);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.navigate(delta);
                }
            });
        }
        for (button, delta) in [(&zoom_out, -1), (&zoom_in, 1)] {
            let weak = Rc::downgrade(&inner);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.zoom_by(delta);
                }
            });
        }
        let weak = Rc::downgrade(&inner);
        inner.fit.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.fit_width();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.page.connect_activate(move |entry| {
            if let Some(view) = weak.upgrade() {
                if let Ok(page) = entry.text().trim().parse::<usize>() {
                    view.go_to(page.saturating_sub(1));
                }
                view.sync_controls();
                view.scroll.grab_focus();
            }
        });
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&inner);
        click.connect_pressed(move |_, _, _, _| {
            if let Some(view) = weak.upgrade() {
                view.scroll.grab_focus();
            }
        });
        inner.canvas.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&inner);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            weak.upgrade()
                .map_or(glib::Propagation::Proceed, |view| view.key(key, modifiers))
        });
        root.add_controller(keys);
        for adjustment in [inner.scroll.vadjustment(), inner.scroll.hadjustment()] {
            let weak = Rc::downgrade(&inner);
            adjustment.connect_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.schedule();
                }
            });
            let weak = Rc::downgrade(&inner);
            adjustment.connect_value_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.schedule();
                }
            });
        }
        let weak = Rc::downgrade(&inner);
        root.connect_map(move |_| {
            if let Some(view) = weak.upgrade() {
                view.schedule();
            }
        });
        let weak = Rc::downgrade(&inner);
        root.connect_scale_factor_notify(move |_| {
            if let Some(view) = weak.upgrade() {
                view.schedule();
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&inner);
        focus.connect_leave(move |_| {
            if let Some(view) = weak.upgrade() {
                view.schedule();
            }
        });
        inner.page.add_controller(focus);
        let weak = Rc::downgrade(&inner);
        root.connect_unmap(move |_| {
            if let Some(view) = weak.upgrade()
                && let Some(document) = view.document.borrow_mut().as_mut()
            {
                document.cancel.cancel();
                document.results = None;
                document.pending.clear();
            }
        });
        Self { root, inner }
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn render(&self, source: &PdfDocument) {
        self.clear();
        self.inner
            .count
            .set_label(&format!("/ {}", source.pages.len()));
        self.inner
            .bookmarks
            .set_sensitive(!source.bookmarks.is_empty());
        self.inner
            .bookmarks
            .set_tooltip_text(Some(if source.bookmarks.is_empty() {
                "This PDF has no bookmarks"
            } else {
                "Document bookmarks"
            }));
        for bookmark in source.bookmarks.iter() {
            let button = gtk::Button::new();
            button.add_css_class("flat");
            let title = gtk::Label::new(Some(&bookmark.title));
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_max_width_chars(36);
            title.set_margin_start(bookmark.depth.min(6) as i32 * 10);
            button.set_child(Some(&title));
            button.set_sensitive(bookmark.page.is_some());
            button.set_tooltip_text(Some(&bookmark.page.map_or_else(
                || format!("{} · destination unavailable", bookmark.title),
                |page| format!("{} · Page {}", bookmark.title, page + 1),
            )));
            let weak = Rc::downgrade(&self.inner);
            let page = bookmark.page;
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade()
                    && let Some(page) = page
                {
                    view.go_to(page);
                    view.bookmarks.popdown();
                    view.scroll.grab_focus();
                }
            });
            self.inner.outline.append(&button);
        }
        *self.inner.document.borrow_mut() = Some(DocumentView {
            source: source.clone(),
            scale: 0.5,
            pixel_ratio: 1,
            fit_width: true,
            viewport_width: 0,
            layout: Layout::default(),
            current: 0,
            jump: Some(0),
            slots: HashMap::new(),
            cache: VecDeque::new(),
            cache_bytes: 0,
            failed: HashSet::new(),
            pending: HashSet::new(),
            results: None,
            cancel: CancelToken::new(),
        });
        self.inner.sync_controls();
        self.inner.schedule();
    }
}

impl View {
    fn schedule(self: &Rc<Self>) {
        if self.scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(16), move || {
            if let Some(view) = weak.upgrade() {
                view.scheduled.set(false);
                view.tick();
            }
        });
    }

    fn clear(&self) {
        if let Some(document) = self.document.borrow_mut().take() {
            document.cancel.cancel();
        }
        while let Some(child) = self.canvas.first_child() {
            self.canvas.remove(&child);
        }
        while let Some(child) = self.outline.first_child() {
            self.outline.remove(&child);
        }
        self.canvas.set_size_request(1, 1);
    }

    fn go_to(self: &Rc<Self>, page: usize) {
        if let Some(document) = self.document.borrow_mut().as_mut() {
            document.jump = Some(page.min(document.source.pages.len() - 1));
        }
        self.schedule();
    }

    fn navigate(self: &Rc<Self>, delta: i32) {
        if let Some(document) = self.document.borrow_mut().as_mut() {
            let page = document.jump.unwrap_or(document.current);
            document.jump = Some(
                (page as i64 + i64::from(delta)).clamp(0, document.source.pages.len() as i64 - 1)
                    as usize,
            );
        }
        self.schedule();
    }

    fn zoom_by(self: &Rc<Self>, delta: i32) {
        if let Some(document) = self.document.borrow_mut().as_mut() {
            document.fit_width = false;
            let scale = (document.scale * 1.2_f32.powi(delta)).clamp(MIN_SCALE, MAX_SCALE);
            self.rescale(document, scale, document.pixel_ratio);
        }
        self.schedule();
    }

    fn fit_width(self: &Rc<Self>) {
        if let Some(document) = self.document.borrow_mut().as_mut() {
            document.fit_width = true;
            document.viewport_width = 0;
        }
        self.schedule();
    }

    fn rescale(&self, document: &mut DocumentView, scale: f32, pixel_ratio: i32) {
        let top = self.scroll.vadjustment().value();
        let page = document.layout.page_at(top);
        let fraction = document.layout.tops.get(page).map_or(0.0, |page_top| {
            ((top - page_top) / f64::from(document.layout.heights[page])).clamp(0.0, 1.0)
        });
        document.scale = scale;
        document.pixel_ratio = pixel_ratio;
        document.cancel.cancel();
        document.results = None;
        document.pending.clear();
        document.cache.clear();
        document.cache_bytes = 0;
        document.failed.clear();
        for slot in document.slots.values_mut() {
            slot.ready = false;
        }
        // Keep existing textures until replacements arrive; zoom never blanks the page.
        document.layout = Layout::new(&document.source.pages, scale, document.viewport_width);
        self.canvas
            .set_size_request(document.layout.width, document.layout.height);
        let horizontal = self.scroll.hadjustment();
        let viewport = f64::from(document.viewport_width);
        horizontal.configure(
            (f64::from(document.layout.width) - viewport).max(0.0) / 2.0,
            0.0,
            f64::from(document.layout.width).max(viewport),
            40.0,
            viewport * 0.9,
            viewport,
        );
        let target =
            document.layout.tops[page] + f64::from(document.layout.heights[page]) * fraction;
        let adjustment = self.scroll.vadjustment();
        adjustment.configure(
            target,
            0.0,
            f64::from(document.layout.height).max(adjustment.page_size()),
            40.0,
            adjustment.page_size() * 0.9,
            adjustment.page_size(),
        );
    }

    fn tick(self: &Rc<Self>) {
        if !self.root.is_mapped() || self.scroll.width() < 1 {
            return;
        }
        let mut state = self.document.borrow_mut();
        let Some(document) = state.as_mut() else {
            return;
        };
        let width = self.scroll.hadjustment().page_size().round() as i32;
        if width < 1 {
            return;
        }
        // Wrap the groups instead of widening the inspector or clipping controls.
        let (_, navigation_width, _, _) = self.navigation.measure(gtk::Orientation::Horizontal, -1);
        let (_, zoom_width, _, _) = self.zoom_bar.measure(gtk::Orientation::Horizontal, -1);
        let wide = width >= navigation_width + zoom_width + 20;
        self.toolbar.set_orientation(if wide {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        });
        self.navigation.set_halign(if wide {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        });
        self.zoom_bar.set_halign(if wide {
            gtk::Align::End
        } else {
            gtk::Align::Center
        });
        let ratio = self.root.scale_factor();
        if document.viewport_width != width || document.pixel_ratio != ratio {
            document.viewport_width = width;
            let scale = if document.fit_width {
                fitted_scale(&document.source.pages, width)
            } else {
                document.scale
            };
            self.rescale(document, scale, ratio);
        }
        let adjustment = self.scroll.vadjustment();
        if let Some(page) = document.jump.take() {
            adjustment.set_value((document.layout.tops[page] - GAP).max(0.0));
        }
        let top = adjustment.value();
        let height = adjustment.page_size().max(1.0);
        document.current = document.layout.page_at(top + (height * 0.5).min(100.0));
        let nearby = document.layout.nearby(top, height);
        document.slots.retain(|index, slot| {
            if nearby.contains(index) {
                true
            } else {
                self.canvas.remove(&slot.root);
                false
            }
        });
        for index in nearby.clone() {
            let slot = document.slots.entry(index).or_insert_with(|| {
                let picture: PageImage = glib::Object::new();
                let root = gtk::Overlay::new();
                root.add_css_class("pdf-page");
                root.set_child(Some(&picture));
                let label = gtk::Label::new(Some(&format!("Page {}", index + 1)));
                label.add_css_class("pdf-page-placeholder");
                label.set_valign(gtk::Align::Center);
                root.add_overlay(&label);
                root.update_property(&[gtk::accessible::Property::Label(&format!(
                    "PDF page {}",
                    index + 1
                ))]);
                self.canvas.put(&root, 0.0, 0.0);
                Slot {
                    root,
                    picture,
                    label,
                    ready: false,
                }
            });
            slot.picture.resize(
                document.layout.widths[index],
                document.layout.heights[index],
            );
            self.canvas.move_(
                &slot.root,
                f64::from(document.layout.width - document.layout.widths[index]) / 2.0,
                document.layout.tops[index],
            );
            if let Some(cached) = document.cache.iter().find(|cached| cached.index == index) {
                slot.picture.set_texture(&cached.texture);
                slot.label.set_visible(false);
                slot.ready = true;
            } else if document.failed.contains(&index) {
                slot.label.set_label("Page unavailable");
            }
        }
        if let Some(receiver) = document.results.as_ref() {
            loop {
                let message = match receiver.try_recv() {
                    Ok(message) => message,
                    Err(error) => {
                        if error.is_disconnected() && !document.pending.is_empty() {
                            document.failed.extend(document.pending.drain());
                            notifications::error(
                                "PDF renderer stopped. Select the file again to retry.",
                            );
                        }
                        break;
                    }
                };
                document.pending.remove(&message.index);
                match message.result {
                    Ok(raster) => {
                        let bytes = raster.rgba.len();
                        let rgba = glib::Bytes::from_owned(raster.rgba);
                        let texture = gdk::MemoryTexture::new(
                            raster.width as i32,
                            raster.height as i32,
                            gdk::MemoryFormat::R8g8b8a8,
                            &rgba,
                            raster.width as usize * 4,
                        );
                        if let Some(slot) = document.slots.get_mut(&message.index) {
                            slot.picture.set_texture(&texture);
                            slot.label.set_visible(false);
                            slot.ready = true;
                        }
                        document.cache.push_back(CachedPage {
                            index: message.index,
                            texture,
                            bytes,
                        });
                        document.cache_bytes += bytes;
                    }
                    Err(error) => {
                        if document.failed.is_empty() {
                            notifications::error(&format!("Could not render PDF page: {error}"));
                        }
                        document.failed.insert(message.index);
                    }
                }
            }
            if receiver.is_empty() && document.pending.is_empty() {
                document.results = None;
            }
        }
        // Prefer retaining visible pages; all raster cache entries share one byte budget.
        while document.cache_bytes > CACHE_BYTES {
            let index = document
                .cache
                .iter()
                .position(|cached| !nearby.contains(&cached.index))
                .unwrap_or(0);
            if let Some(cached) = document.cache.remove(index) {
                document.cache_bytes -= cached.bytes;
            }
        }
        let visible = document.layout.page_at(top)..=(document.layout.page_at(top + height));
        let mut missing: Vec<_> = nearby
            .filter(|index| {
                !document.failed.contains(index)
                    && !document.slots.get(index).is_some_and(|slot| slot.ready)
                    && !document.cache.iter().any(|cached| cached.index == *index)
            })
            .collect();
        missing.sort_by_key(|index| (!visible.contains(index), index.abs_diff(document.current)));
        // Replace obsolete work when a scroll jumps past the pages being rendered.
        if document.results.is_some()
            && !document.pending.iter().any(|index| visible.contains(index))
            && missing.iter().any(|index| visible.contains(index))
        {
            document.cancel.cancel();
            document.results = None;
            document.pending.clear();
        }
        if document.results.is_none() && !missing.is_empty() {
            missing.truncate(64);
            let cancel = CancelToken::new();
            let pages = missing
                .iter()
                .map(|&index| (index, document.scale * ratio as f32))
                .collect();
            match document.source.request(pages, cancel.clone()) {
                Ok(receiver) => {
                    document.cancel = cancel;
                    document.pending = missing.iter().copied().collect();
                    document.results = Some(receiver);
                }
                Err(RequestError::Busy) => {}
                Err(RequestError::Stopped) => {
                    if document.failed.is_empty() {
                        notifications::error(
                            "PDF renderer stopped. Select the file again to retry.",
                        );
                    }
                    document.failed.extend(missing.drain(..));
                }
            }
        }
        if document.results.is_some() || !missing.is_empty() {
            self.schedule();
        }
        drop(state);
        self.sync_controls();
    }

    fn sync_controls(&self) {
        if let Some(document) = self.document.borrow().as_ref() {
            if !self.page.has_focus() && !self.page.has_visible_focus() {
                let focus_in_entry = self
                    .root
                    .root()
                    .and_then(|root| root.focus())
                    .is_some_and(|focus| focus.is_ancestor(&self.page));
                let text = (document.current + 1).to_string();
                if !focus_in_entry && self.page.text() != text {
                    self.page.set_text(&text);
                }
            }
            self.previous.set_sensitive(document.current > 0);
            self.next
                .set_sensitive(document.current + 1 < document.source.pages.len());
            self.zoom
                .set_label(&format!("{:.0}%", document.scale * 100.0));
            self.fit.set_active(document.fit_width);
        }
    }

    fn key(self: &Rc<Self>, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        if control && matches!(key, gdk::Key::l | gdk::Key::L) {
            self.page.grab_focus();
            self.page.select_region(0, -1);
            return glib::Propagation::Stop;
        }
        if self
            .root
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| focus.is::<gtk::Text>() || focus.is::<gtk::Entry>())
        {
            return glib::Propagation::Proceed;
        }
        if control && matches!(key, gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add) {
            self.zoom_by(1);
        } else if control && matches!(key, gdk::Key::minus | gdk::Key::KP_Subtract) {
            self.zoom_by(-1);
        } else if control && matches!(key, gdk::Key::_0 | gdk::Key::KP_0) {
            self.fit_width();
        } else if alt && matches!(key, gdk::Key::Up | gdk::Key::Down) {
            self.navigate(if key == gdk::Key::Up { -1 } else { 1 });
        } else if key == gdk::Key::Home {
            self.go_to(0);
        } else if key == gdk::Key::End {
            self.go_to(usize::MAX);
        } else if matches!(
            key,
            gdk::Key::Page_Up | gdk::Key::Page_Down | gdk::Key::Up | gdk::Key::Down
        ) {
            let adjustment = self.scroll.vadjustment();
            let distance = if matches!(key, gdk::Key::Up | gdk::Key::Down) {
                40.0
            } else {
                adjustment.page_size() * 0.9
            };
            let direction = if matches!(key, gdk::Key::Up | gdk::Key::Page_Up) {
                -1.0
            } else {
                1.0
            };
            adjustment.set_value(adjustment.value() + distance * direction);
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    }
}

fn fitted_scale(pages: &[PageSize], width: i32) -> f32 {
    let widest = pages.iter().map(|page| page.width).fold(1.0, f32::max);
    ((width - 2 * GAP as i32).max(1) as f32 / widest).clamp(MIN_SCALE, MAX_SCALE)
}

fn button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("pdf-preview-button");
    button.set_tooltip_text(Some(tooltip));
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

// Unlike Picture, this widget's natural size is the logical page size, independent
// of raster DPI. This keeps 2× displays and mixed page sizes aligned while zooming.
mod page_image {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct PageImage {
        pub size: Cell<(i32, i32)>,
        pub texture: RefCell<Option<gdk::MemoryTexture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PageImage {
        const NAME: &'static str = "CommanderPdfPageImage";
        type Type = super::PageImage;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for PageImage {}
    impl WidgetImpl for PageImage {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let (width, height) = self.size.get();
            let size = if orientation == gtk::Orientation::Horizontal {
                width
            } else {
                height
            };
            (size, size, -1, -1)
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(texture) = self.texture.borrow().as_ref() {
                snapshot.append_texture(
                    texture,
                    &gtk::graphene::Rect::new(
                        0.0,
                        0.0,
                        self.obj().width() as f32,
                        self.obj().height() as f32,
                    ),
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct PageImage(ObjectSubclass<page_image::PageImage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PageImage {
    fn resize(&self, width: i32, height: i32) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        if self.imp().size.replace((width, height)) != (width, height) {
            self.queue_resize();
        }
    }
    fn set_texture(&self, texture: &gdk::MemoryTexture) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        if self.imp().texture.borrow().as_ref() != Some(texture) {
            *self.imp().texture.borrow_mut() = Some(texture.clone());
            self.queue_draw();
        }
    }
}

//! The embedded PTY terminal: VT screen state, drawing, key encoding, and its tab strip.

use super::*;

#[path = "terminal_tabs.rs"]
mod terminal_tabs;
use terminal_tabs::TerminalTabs;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TerminalPoint {
    pub(super) row: u16,
    pub(super) col: u16,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalSelection {
    pub(super) anchor: TerminalPoint,
    pub(super) focus: TerminalPoint,
}

pub(super) struct TerminalScreenState {
    pub(super) parser: vt100::Parser,
    pub(super) selection: Option<TerminalSelection>,
    pub(super) revision: u64,
}

impl TerminalScreenState {
    pub(super) fn new() -> Self {
        Self {
            parser: vt100::Parser::new(28, 120, 10_000),
            selection: None,
            revision: 0,
        }
    }

    pub(super) fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn resize(&mut self, rows: u16, cols: u16) -> bool {
        if self.parser.screen().size() == (rows, cols) {
            return false;
        }
        self.parser.screen_mut().set_size(rows, cols);
        self.selection = None;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let (start, end) = if selection.anchor <= selection.focus {
            (selection.anchor, selection.focus)
        } else {
            (selection.focus, selection.anchor)
        };
        let (rows, cols) = self.parser.screen().size();
        if rows == 0 || cols == 0 {
            return None;
        }
        let end_col = end.col.saturating_add(1).min(cols);
        let text = self.parser.screen().contents_between(
            start.row.min(rows - 1),
            start.col,
            end.row.min(rows - 1),
            end_col,
        );
        (!text.is_empty()).then_some(text)
    }
}

pub(super) fn terminal_title(path: &VPath) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("/")
        .to_owned()
}

pub(super) fn replacement_terminal_index(remaining: usize, removed: usize) -> Option<usize> {
    (remaining > 0).then(|| removed.min(remaining - 1))
}

pub(super) const TERMINAL_FONT_SIZE: f64 = 13.0;
pub(super) const TERMINAL_CELL_WIDTH: f64 = 8.0;
pub(super) const TERMINAL_CELL_HEIGHT: f64 = 18.0;
pub(super) const TERMINAL_PADDING_X: f64 = 10.0;
pub(super) const TERMINAL_PADDING_Y: f64 = 7.0;

pub(super) type ActiveTerminalScreen = Rc<RefCell<Option<Rc<RefCell<TerminalScreenState>>>>>;

pub(super) fn terminal_dimensions(width: i32, height: i32) -> (u16, u16) {
    let usable_width = (f64::from(width) - TERMINAL_PADDING_X * 2.0).max(1.0);
    let usable_height = (f64::from(height) - TERMINAL_PADDING_Y * 2.0).max(1.0);
    let cols = (usable_width / TERMINAL_CELL_WIDTH)
        .floor()
        .clamp(2.0, f64::from(u16::MAX)) as u16;
    let rows = (usable_height / TERMINAL_CELL_HEIGHT)
        .floor()
        .clamp(2.0, f64::from(u16::MAX)) as u16;
    (rows, cols)
}

pub(super) fn terminal_point_at(screen: &TerminalScreenState, x: f64, y: f64) -> TerminalPoint {
    let (rows, cols) = screen.parser.screen().size();
    let col = ((x - TERMINAL_PADDING_X).max(0.0) / TERMINAL_CELL_WIDTH).floor() as u16;
    let row = ((y - TERMINAL_PADDING_Y).max(0.0) / TERMINAL_CELL_HEIGHT).floor() as u16;
    TerminalPoint {
        row: row.min(rows.saturating_sub(1)),
        col: col.min(cols.saturating_sub(1)),
    }
}

pub(super) fn terminal_point_selected(selection: TerminalSelection, point: TerminalPoint) -> bool {
    let (start, end) = if selection.anchor <= selection.focus {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    };
    point >= start && point <= end
}

pub(super) fn terminal_rgba(
    area: &gtk::DrawingArea,
    name: &str,
    fallback: (f64, f64, f64),
) -> (f64, f64, f64) {
    #[allow(deprecated)]
    area.style_context()
        .lookup_color(name)
        .map_or(fallback, |color| {
            (
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
            )
        })
}

pub(super) fn terminal_ansi_color(
    color: vt100::Color,
    fallback: (f64, f64, f64),
    bright: bool,
) -> (f64, f64, f64) {
    let index = match color {
        vt100::Color::Default => return fallback,
        vt100::Color::Rgb(red, green, blue) => {
            return (
                f64::from(red) / 255.0,
                f64::from(green) / 255.0,
                f64::from(blue) / 255.0,
            );
        }
        vt100::Color::Idx(index) => {
            if bright && index < 8 {
                index + 8
            } else {
                index
            }
        }
    };
    const ANSI: [(u8, u8, u8); 16] = [
        (0x1d, 0x20, 0x21),
        (0xcc, 0x66, 0x66),
        (0xb5, 0xbd, 0x68),
        (0xf0, 0xc6, 0x74),
        (0x81, 0xa2, 0xbe),
        (0xb2, 0x94, 0xbb),
        (0x8a, 0xbe, 0xb7),
        (0xc5, 0xc8, 0xc6),
        (0x66, 0x66, 0x66),
        (0xd5, 0x4e, 0x53),
        (0xb9, 0xca, 0x4a),
        (0xe7, 0xc5, 0x47),
        (0x7a, 0xa6, 0xda),
        (0xc3, 0x97, 0xd8),
        (0x70, 0xc0, 0xb1),
        (0xea, 0xea, 0xea),
    ];
    let (red, green, blue) = if index < 16 {
        ANSI[usize::from(index)]
    } else if index < 232 {
        let cube = index - 16;
        let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
        (level(cube / 36), level((cube % 36) / 6), level(cube % 6))
    } else {
        let gray = 8 + (index - 232) * 10;
        (gray, gray, gray)
    };
    (
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
    )
}

pub(super) fn draw_terminal(
    area: &gtk::DrawingArea,
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    active: &ActiveTerminalScreen,
) {
    let background = terminal_rgba(area, "carelo_void", (0.075, 0.086, 0.082));
    let foreground = terminal_rgba(area, "carelo_text", (0.84, 0.86, 0.84));
    let accent = terminal_rgba(area, "carelo_accent", (0.10, 0.55, 0.96));
    context.set_source_rgb(background.0, background.1, background.2);
    context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
    let _ = context.fill();

    let active = active.borrow();
    let Some(screen) = active.as_ref() else {
        return;
    };
    let screen = screen.borrow();
    let terminal = screen.parser.screen();
    let (rows, cols) = terminal.size();
    let selection = screen.selection;
    let cursor =
        (terminal.scrollback() == 0 && !terminal.hide_cursor()).then(|| terminal.cursor_position());

    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = terminal.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let point = TerminalPoint { row, col };
            let selected = selection.is_some_and(|value| terminal_point_selected(value, point));
            let cursor_cell = cursor == Some((row, col));
            let mut cell_foreground = terminal_ansi_color(cell.fgcolor(), foreground, cell.bold());
            let mut cell_background = terminal_ansi_color(cell.bgcolor(), background, false);
            if cell.inverse() {
                std::mem::swap(&mut cell_foreground, &mut cell_background);
            }
            if selected {
                cell_background = (
                    accent.0 * 0.55 + background.0 * 0.45,
                    accent.1 * 0.55 + background.1 * 0.45,
                    accent.2 * 0.55 + background.2 * 0.45,
                );
            }
            if cursor_cell && area.has_focus() {
                cell_background = accent;
                cell_foreground = background;
            }
            if selected || cursor_cell || cell.bgcolor() != vt100::Color::Default {
                context.set_source_rgb(cell_background.0, cell_background.1, cell_background.2);
                context.rectangle(
                    TERMINAL_PADDING_X + f64::from(col) * TERMINAL_CELL_WIDTH,
                    TERMINAL_PADDING_Y + f64::from(row) * TERMINAL_CELL_HEIGHT,
                    if cell.is_wide() {
                        TERMINAL_CELL_WIDTH * 2.0
                    } else {
                        TERMINAL_CELL_WIDTH
                    },
                    TERMINAL_CELL_HEIGHT,
                );
                let _ = context.fill();
            }
            if cell.contents().is_empty() {
                continue;
            }
            if cell.dim() {
                cell_foreground = (
                    cell_foreground.0 * 0.65,
                    cell_foreground.1 * 0.65,
                    cell_foreground.2 * 0.65,
                );
            }
            context.select_font_face(
                "monospace",
                if cell.italic() {
                    gtk::cairo::FontSlant::Italic
                } else {
                    gtk::cairo::FontSlant::Normal
                },
                if cell.bold() {
                    gtk::cairo::FontWeight::Bold
                } else {
                    gtk::cairo::FontWeight::Normal
                },
            );
            context.set_font_size(TERMINAL_FONT_SIZE);
            context.set_source_rgb(cell_foreground.0, cell_foreground.1, cell_foreground.2);
            let x = TERMINAL_PADDING_X + f64::from(col) * TERMINAL_CELL_WIDTH;
            let y =
                TERMINAL_PADDING_Y + f64::from(row) * TERMINAL_CELL_HEIGHT + TERMINAL_CELL_HEIGHT
                    - 4.0;
            context.move_to(x, y);
            let _ = context.show_text(cell.contents());
            if cell.underline() {
                context.set_line_width(1.0);
                context.move_to(x, y + 2.0);
                context.line_to(
                    x + if cell.is_wide() {
                        TERMINAL_CELL_WIDTH * 2.0
                    } else {
                        TERMINAL_CELL_WIDTH
                    },
                    y + 2.0,
                );
                let _ = context.stroke();
            }
        }
    }
    if let Some((row, col)) = cursor
        && !area.has_focus()
    {
        context.set_source_rgb(accent.0, accent.1, accent.2);
        context.set_line_width(1.0);
        context.rectangle(
            TERMINAL_PADDING_X + f64::from(col) * TERMINAL_CELL_WIDTH + 0.5,
            TERMINAL_PADDING_Y + f64::from(row) * TERMINAL_CELL_HEIGHT + 0.5,
            TERMINAL_CELL_WIDTH - 1.0,
            TERMINAL_CELL_HEIGHT - 1.0,
        );
        let _ = context.stroke();
    }
}

pub(super) fn terminal_key_bytes(
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    application_cursor: bool,
) -> Option<Vec<u8>> {
    let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
    let modifier_parameter = 1 + u8::from(shift) + u8::from(alt) * 2 + u8::from(control) * 4;
    let mut bytes = match key {
        gdk::Key::Return | gdk::Key::KP_Enter => vec![b'\r'],
        gdk::Key::BackSpace => vec![0x7f],
        gdk::Key::Tab if shift => b"\x1b[Z".to_vec(),
        gdk::Key::Tab => vec![b'\t'],
        gdk::Key::ISO_Left_Tab => b"\x1b[Z".to_vec(),
        gdk::Key::Escape => vec![0x1b],
        gdk::Key::Up | gdk::Key::KP_Up => {
            terminal_cursor_sequence(b'A', application_cursor, modifier_parameter)
        }
        gdk::Key::Down | gdk::Key::KP_Down => {
            terminal_cursor_sequence(b'B', application_cursor, modifier_parameter)
        }
        gdk::Key::Right | gdk::Key::KP_Right => {
            terminal_cursor_sequence(b'C', application_cursor, modifier_parameter)
        }
        gdk::Key::Left | gdk::Key::KP_Left => {
            terminal_cursor_sequence(b'D', application_cursor, modifier_parameter)
        }
        gdk::Key::Home | gdk::Key::KP_Home => {
            terminal_cursor_sequence(b'H', false, modifier_parameter)
        }
        gdk::Key::End | gdk::Key::KP_End => {
            terminal_cursor_sequence(b'F', false, modifier_parameter)
        }
        gdk::Key::Insert | gdk::Key::KP_Insert => b"\x1b[2~".to_vec(),
        gdk::Key::Delete | gdk::Key::KP_Delete => b"\x1b[3~".to_vec(),
        gdk::Key::Page_Up | gdk::Key::KP_Page_Up => b"\x1b[5~".to_vec(),
        gdk::Key::Page_Down | gdk::Key::KP_Page_Down => b"\x1b[6~".to_vec(),
        gdk::Key::F1 => b"\x1bOP".to_vec(),
        gdk::Key::F2 => b"\x1bOQ".to_vec(),
        gdk::Key::F3 => b"\x1bOR".to_vec(),
        gdk::Key::F4 => b"\x1bOS".to_vec(),
        gdk::Key::F5 => b"\x1b[15~".to_vec(),
        gdk::Key::F6 => b"\x1b[17~".to_vec(),
        gdk::Key::F7 => b"\x1b[18~".to_vec(),
        gdk::Key::F8 => b"\x1b[19~".to_vec(),
        gdk::Key::F9 => b"\x1b[20~".to_vec(),
        gdk::Key::F10 => b"\x1b[21~".to_vec(),
        gdk::Key::F11 => b"\x1b[23~".to_vec(),
        gdk::Key::F12 => b"\x1b[24~".to_vec(),
        _ if control => {
            let character = key.to_unicode()?;
            let value = match character.to_ascii_lowercase() {
                'a'..='z' => u8::try_from(character.to_ascii_lowercase() as u32).ok()? - b'a' + 1,
                '@' | ' ' => 0,
                '[' => 27,
                '\\' => 28,
                ']' => 29,
                '^' => 30,
                '_' => 31,
                '?' => 127,
                _ => return None,
            };
            vec![value]
        }
        _ if !modifiers
            .intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK) =>
        {
            let character = key.to_unicode()?;
            if character.is_control() {
                return None;
            }
            let mut encoded = [0_u8; 4];
            character.encode_utf8(&mut encoded).as_bytes().to_vec()
        }
        _ => return None,
    };
    if alt
        && bytes.first() != Some(&0x1b)
        && !matches!(
            key,
            gdk::Key::Up
                | gdk::Key::KP_Up
                | gdk::Key::Down
                | gdk::Key::KP_Down
                | gdk::Key::Right
                | gdk::Key::KP_Right
                | gdk::Key::Left
                | gdk::Key::KP_Left
                | gdk::Key::Home
                | gdk::Key::KP_Home
                | gdk::Key::End
                | gdk::Key::KP_End
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

pub(super) fn terminal_cursor_sequence(
    final_byte: u8,
    application_cursor: bool,
    modifier_parameter: u8,
) -> Vec<u8> {
    if modifier_parameter > 1 {
        return format!("\x1b[1;{}{}", modifier_parameter, char::from(final_byte)).into_bytes();
    }
    vec![
        0x1b,
        if application_cursor { b'O' } else { b'[' },
        final_byte,
    ]
}

pub(super) fn scroll_terminal(
    active: &ActiveTerminalScreen,
    area: &gtk::DrawingArea,
    direction: i32,
    amount: usize,
) {
    let active = active.borrow();
    let Some(screen) = active.as_ref() else {
        return;
    };
    let mut screen = screen.borrow_mut();
    let current = screen.parser.screen().scrollback();
    let target = if direction < 0 {
        current.saturating_add(amount)
    } else {
        current.saturating_sub(amount)
    };
    screen.parser.screen_mut().set_scrollback(target);
    screen.revision = screen.revision.wrapping_add(1);
    area.queue_draw();
}

pub(super) fn copy_terminal_selection(active: &ActiveTerminalScreen) {
    let selected = active
        .borrow()
        .as_ref()
        .and_then(|screen| screen.borrow().selected_text());
    if let Some(text) = selected
        && let Some(display) = gdk::Display::default()
    {
        display.clipboard().set_text(&text);
    }
}

pub(super) fn paste_terminal_clipboard(
    active: &ActiveTerminalScreen,
    input: &relm4::Sender<AppMsg>,
) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let bracketed = active
        .borrow()
        .as_ref()
        .is_some_and(|screen| screen.borrow().parser.screen().bracketed_paste());
    let input = input.clone();
    display
        .clipboard()
        .read_text_async(None::<&gio::Cancellable>, move |result| {
            if let Ok(Some(text)) = result {
                let mut bytes = text.as_bytes().to_vec();
                if bracketed {
                    bytes.splice(..0, b"\x1b[200~".iter().copied());
                    bytes.extend_from_slice(b"\x1b[201~");
                }
                let _ = input.send(AppMsg::TerminalInput(bytes));
            }
        });
}

pub(super) const DEFAULT_TERMINAL_HEIGHT: i32 = 270;
pub(super) const MIN_TERMINAL_HEIGHT: i32 = 110;
pub(super) const MIN_WORKSPACE_HEIGHT: i32 = 120;

pub(super) fn terminal_split_position(total_height: i32, requested_terminal_height: i32) -> i32 {
    if total_height <= 0 {
        return 0;
    }
    let maximum_terminal_height = total_height.saturating_sub(MIN_WORKSPACE_HEIGHT).max(1);
    let minimum_terminal_height = MIN_TERMINAL_HEIGHT.min(maximum_terminal_height);
    let terminal_height =
        requested_terminal_height.clamp(minimum_terminal_height, maximum_terminal_height);
    total_height.saturating_sub(terminal_height)
}

pub(super) fn apply_terminal_split_height(split: &gtk::Paned, terminal_height: i32) -> bool {
    if split.height() <= 0 {
        return false;
    }
    split.set_position(terminal_split_position(split.height(), terminal_height));
    true
}

pub(super) struct TerminalWidgets {
    pub(super) root: gtk::Paned,
    pub(super) panel: gtk::Revealer,
    tabs: TerminalTabs,
    pub(super) surface: gtk::DrawingArea,
    pub(super) active_screen: ActiveTerminalScreen,
    pub(super) active_id: Rc<Cell<Option<u64>>>,
    pub(super) rendered_screen_revision: Cell<u64>,
    pub(super) rendered_terminal: Cell<Option<u64>>,
    pub(super) rendered_revision: Cell<u64>,
    pub(super) rendered_visible: Cell<bool>,
    pub(super) remembered_height: Rc<Cell<i32>>,
    pub(super) panel_open: Rc<Cell<bool>>,
}

impl TerminalWidgets {
    pub(super) fn new(
        sender: &ComponentSender<AppModel>,
        workspace: &impl IsA<gtk::Widget>,
    ) -> Self {
        let panel_revealer = gtk::Revealer::new();
        panel_revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        panel_revealer.set_transition_duration(180);
        panel_revealer.set_hexpand(true);
        panel_revealer.set_visible(false);
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("terminal-panel");
        panel.set_height_request(MIN_TERMINAL_HEIGHT);
        let tabs = TerminalTabs::new(sender.input_sender());
        panel.append(&tabs.root);
        let surface = gtk::DrawingArea::new();
        surface.update_property(&[gtk::accessible::Property::Label("Interactive terminal")]);
        surface.add_css_class("terminal-surface");
        surface.set_focusable(true);
        surface.set_focus_on_click(true);
        surface.set_hexpand(true);
        surface.set_vexpand(true);
        surface.set_cursor_from_name(Some("text"));
        let active_screen: ActiveTerminalScreen = Rc::new(RefCell::new(None));
        let active_id = Rc::new(Cell::new(None));
        {
            let active_screen = Rc::clone(&active_screen);
            surface.set_draw_func(move |area, context, width, height| {
                draw_terminal(area, context, width, height, &active_screen);
            });
        }
        {
            let active_id = Rc::clone(&active_id);
            let input = sender.input_sender().clone();
            surface.connect_resize(move |_, width, height| {
                if let Some(id) = active_id.get() {
                    let (rows, cols) = terminal_dimensions(width, height);
                    let _ = input.send(AppMsg::ResizeTerminal { id, rows, cols });
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let active_screen = Rc::clone(&active_screen);
            let input = sender.input_sender().clone();
            let surface = surface.clone();
            keys.connect_key_pressed(move |_, key, _, modifiers| {
                let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                if control && shift && key == gdk::Key::C {
                    copy_terminal_selection(&active_screen);
                    return glib::Propagation::Stop;
                }
                if control && shift && key == gdk::Key::V {
                    paste_terminal_clipboard(&active_screen, &input);
                    return glib::Propagation::Stop;
                }
                if shift && matches!(key, gdk::Key::Page_Up | gdk::Key::KP_Page_Up) {
                    let rows = active_screen.borrow().as_ref().map_or(10, |screen| {
                        usize::from(screen.borrow().parser.screen().size().0)
                    });
                    scroll_terminal(&active_screen, &surface, -1, rows.saturating_sub(1));
                    return glib::Propagation::Stop;
                }
                if shift && matches!(key, gdk::Key::Page_Down | gdk::Key::KP_Page_Down) {
                    let rows = active_screen.borrow().as_ref().map_or(10, |screen| {
                        usize::from(screen.borrow().parser.screen().size().0)
                    });
                    scroll_terminal(&active_screen, &surface, 1, rows.saturating_sub(1));
                    return glib::Propagation::Stop;
                }
                let application_cursor = active_screen
                    .borrow()
                    .as_ref()
                    .is_some_and(|screen| screen.borrow().parser.screen().application_cursor());
                if let Some(bytes) = terminal_key_bytes(key, modifiers, application_cursor) {
                    let _ = input.send(AppMsg::TerminalInput(bytes));
                    surface.queue_draw();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        surface.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        {
            let surface = surface.clone();
            focus.connect_enter(move |_| surface.queue_draw());
        }
        {
            let surface = surface.clone();
            focus.connect_leave(move |_| surface.queue_draw());
        }
        surface.add_controller(focus);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        {
            let active_screen = Rc::clone(&active_screen);
            let surface = surface.clone();
            scroll.connect_scroll(move |_, _, dy| {
                if dy.abs() < f64::EPSILON {
                    return glib::Propagation::Proceed;
                }
                scroll_terminal(&active_screen, &surface, if dy < 0.0 { -1 } else { 1 }, 3);
                glib::Propagation::Stop
            });
        }
        surface.add_controller(scroll);

        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        let drag_origin = Rc::new(Cell::new(None::<(f64, f64)>));
        {
            let active_screen = Rc::clone(&active_screen);
            let drag_origin = Rc::clone(&drag_origin);
            let surface = surface.clone();
            drag.connect_drag_begin(move |_, x, y| {
                surface.grab_focus();
                drag_origin.set(Some((x, y)));
                if let Some(screen) = active_screen.borrow().as_ref() {
                    let mut screen = screen.borrow_mut();
                    let point = terminal_point_at(&screen, x, y);
                    screen.selection = Some(TerminalSelection {
                        anchor: point,
                        focus: point,
                    });
                    screen.revision = screen.revision.wrapping_add(1);
                    surface.queue_draw();
                }
            });
        }
        {
            let active_screen = Rc::clone(&active_screen);
            let drag_origin = Rc::clone(&drag_origin);
            let surface = surface.clone();
            drag.connect_drag_update(move |_, offset_x, offset_y| {
                let Some((start_x, start_y)) = drag_origin.get() else {
                    return;
                };
                if let Some(screen) = active_screen.borrow().as_ref() {
                    let mut screen = screen.borrow_mut();
                    let point = terminal_point_at(&screen, start_x + offset_x, start_y + offset_y);
                    if let Some(selection) = screen.selection.as_mut() {
                        selection.focus = point;
                    }
                    screen.revision = screen.revision.wrapping_add(1);
                    surface.queue_draw();
                }
            });
        }
        {
            let active_screen = Rc::clone(&active_screen);
            let drag_origin = Rc::clone(&drag_origin);
            let surface = surface.clone();
            drag.connect_drag_end(move |_, offset_x, offset_y| {
                drag_origin.set(None);
                if offset_x.abs() < 3.0
                    && offset_y.abs() < 3.0
                    && let Some(screen) = active_screen.borrow().as_ref()
                {
                    let mut screen = screen.borrow_mut();
                    screen.selection = None;
                    screen.revision = screen.revision.wrapping_add(1);
                    surface.queue_draw();
                }
            });
        }
        surface.add_controller(drag);

        let context_menu = gtk::Popover::new();
        context_menu.add_css_class("context-menu");
        context_menu.set_autohide(true);
        context_menu.set_has_arrow(false);
        context_menu.set_parent(&surface);
        let context_actions = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let copy = gtk::Button::with_label("Copy");
        let paste = gtk::Button::with_label("Paste");
        let select_all = gtk::Button::with_label("Select All");
        let clear = gtk::Button::with_label("Clear Terminal");
        for button in [&copy, &paste, &select_all, &clear] {
            button.add_css_class("flat");
            button.add_css_class("context-menu-item");
            button.set_halign(gtk::Align::Fill);
            context_actions.append(button);
        }
        {
            let active_screen = Rc::clone(&active_screen);
            let context_menu = context_menu.clone();
            copy.connect_clicked(move |_| {
                copy_terminal_selection(&active_screen);
                context_menu.popdown();
            });
        }
        {
            let active_screen = Rc::clone(&active_screen);
            let input = sender.input_sender().clone();
            let context_menu = context_menu.clone();
            paste.connect_clicked(move |_| {
                paste_terminal_clipboard(&active_screen, &input);
                context_menu.popdown();
            });
        }
        {
            let active_screen = Rc::clone(&active_screen);
            let surface = surface.clone();
            let context_menu = context_menu.clone();
            select_all.connect_clicked(move |_| {
                if let Some(screen) = active_screen.borrow().as_ref() {
                    let mut screen = screen.borrow_mut();
                    let (rows, cols) = screen.parser.screen().size();
                    screen.selection = Some(TerminalSelection {
                        anchor: TerminalPoint { row: 0, col: 0 },
                        focus: TerminalPoint {
                            row: rows.saturating_sub(1),
                            col: cols.saturating_sub(1),
                        },
                    });
                    screen.revision = screen.revision.wrapping_add(1);
                    surface.queue_draw();
                }
                context_menu.popdown();
            });
        }
        {
            let active_screen = Rc::clone(&active_screen);
            let surface = surface.clone();
            let context_menu = context_menu.clone();
            clear.connect_clicked(move |_| {
                if let Some(screen) = active_screen.borrow().as_ref() {
                    let mut screen = screen.borrow_mut();
                    let (rows, cols) = screen.parser.screen().size();
                    screen.parser = vt100::Parser::new(rows, cols, 10_000);
                    screen.selection = None;
                    screen.revision = screen.revision.wrapping_add(1);
                    surface.queue_draw();
                }
                context_menu.popdown();
            });
        }
        context_menu.set_child(Some(&context_actions));
        let context_click = gtk::GestureClick::new();
        context_click.set_button(3);
        {
            let surface = surface.clone();
            context_click.connect_pressed(move |gesture, _, x, y| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                surface.grab_focus();
                context_menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                context_menu.popup();
            });
        }
        surface.add_controller(context_click);
        panel.append(&surface);
        panel_revealer.set_child(Some(&panel));

        let root = gtk::Paned::new(gtk::Orientation::Vertical);
        root.set_widget_name("terminal-split");
        root.add_css_class("terminal-split");
        root.set_wide_handle(true);
        root.set_resize_start_child(true);
        root.set_resize_end_child(false);
        root.set_shrink_start_child(false);
        root.set_shrink_end_child(false);
        root.set_start_child(Some(workspace));
        root.set_end_child(Some(&panel_revealer));
        root.set_hexpand(true);
        root.set_vexpand(true);
        let remembered_height = Rc::new(Cell::new(DEFAULT_TERMINAL_HEIGHT));
        let panel_open = Rc::new(Cell::new(false));
        {
            let remembered_height = Rc::clone(&remembered_height);
            let panel_open = Rc::clone(&panel_open);
            root.connect_position_notify(move |split| {
                if !panel_open.get() || split.height() <= 0 {
                    return;
                }
                let height = split.height().saturating_sub(split.position());
                if height >= MIN_TERMINAL_HEIGHT {
                    remembered_height.set(height);
                }
            });
        }
        Self {
            root,
            panel: panel_revealer,
            tabs,
            surface,
            active_screen,
            active_id,
            rendered_screen_revision: Cell::new(u64::MAX),
            rendered_terminal: Cell::new(None),
            rendered_revision: Cell::new(u64::MAX),
            rendered_visible: Cell::new(false),
            remembered_height,
            panel_open,
        }
    }

    pub(super) fn render(&self, model: &AppModel, sender: &ComponentSender<AppModel>) {
        if self.rendered_visible.replace(model.terminal_visible) != model.terminal_visible {
            if model.terminal_visible {
                self.panel_open.set(true);
                self.panel.set_visible(true);
                self.panel.set_reveal_child(true);
                if !apply_terminal_split_height(&self.root, self.remembered_height.get()) {
                    let split = self.root.clone();
                    let remembered_height = Rc::clone(&self.remembered_height);
                    let panel_open = Rc::clone(&self.panel_open);
                    glib::timeout_add_local_once(Duration::from_millis(40), move || {
                        if panel_open.get() {
                            apply_terminal_split_height(&split, remembered_height.get());
                        }
                    });
                }
                let surface = self.surface.clone();
                glib::timeout_add_local_once(Duration::from_millis(220), move || {
                    surface.grab_focus();
                });
            } else {
                let current_height = self.root.height().saturating_sub(self.root.position());
                if current_height >= MIN_TERMINAL_HEIGHT {
                    self.remembered_height.set(current_height);
                }
                self.panel_open.set(false);
                self.panel.set_reveal_child(false);
                let panel = self.panel.clone();
                let panel_open = Rc::clone(&self.panel_open);
                glib::timeout_add_local_once(Duration::from_millis(180), move || {
                    if !panel_open.get() {
                        panel.set_visible(false);
                    }
                });
            }
        }

        if self.rendered_revision.replace(model.terminal_revision) != model.terminal_revision {
            self.tabs.render(
                &model.terminal_tabs,
                model.active_terminal,
                sender.input_sender(),
                &self.surface,
            );
        }

        let active = model
            .active_terminal
            .and_then(|id| model.terminal_tabs.iter().find(|tab| tab.id == id));
        let active_id = active.map(|tab| tab.id);
        let terminal_changed = self.rendered_terminal.replace(active_id) != active_id;
        self.active_id.set(active_id);
        let screen = active.map(|tab| Rc::clone(&tab.screen));
        let screen_revision = screen.as_ref().map_or(0, |screen| screen.borrow().revision);
        *self.active_screen.borrow_mut() = screen;
        if terminal_changed
            || self.rendered_screen_revision.replace(screen_revision) != screen_revision
        {
            self.surface.queue_draw();
        }
        if terminal_changed && model.terminal_visible {
            self.surface.grab_focus();
            let (rows, cols) = terminal_dimensions(self.surface.width(), self.surface.height());
            if let Some(id) = active_id {
                let _ = sender
                    .input_sender()
                    .send(AppMsg::ResizeTerminal { id, rows, cols });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_terminal_selects_the_nearest_remaining_tab() {
        assert_eq!(replacement_terminal_index(0, 0), None);
        assert_eq!(replacement_terminal_index(2, 0), Some(0));
        assert_eq!(replacement_terminal_index(2, 1), Some(1));
        assert_eq!(replacement_terminal_index(2, 2), Some(1));
    }

    #[test]
    fn terminal_tabs_use_their_starting_folder_as_the_title() {
        assert_eq!(terminal_title(&VPath::from("/")), "/");
        assert_eq!(terminal_title(&VPath::from("/home/artur/DEV")), "DEV");
    }

    #[test]
    fn terminal_screen_preserves_vt_colors_and_cursor_updates() {
        let mut screen = TerminalScreenState::new();
        screen.process(b"\x1b[31mred\x1b[0m\r\nnext");
        assert_eq!(
            screen.parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(screen.parser.screen().cursor_position(), (1, 4));
        assert!(screen.parser.screen().contents().contains("red\nnext"));
    }

    #[test]
    fn terminal_keys_encode_control_and_application_sequences() {
        assert_eq!(
            terminal_key_bytes(gdk::Key::C, gdk::ModifierType::CONTROL_MASK, false,),
            Some(vec![3])
        );
        assert_eq!(
            terminal_key_bytes(gdk::Key::Up, gdk::ModifierType::empty(), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(gdk::Key::Up, gdk::ModifierType::empty(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(gdk::Key::Left, gdk::ModifierType::CONTROL_MASK, false,),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(terminal_dimensions(800, 270), (14, 97));
    }

    #[test]
    fn terminal_split_respects_panel_and_workspace_minimums() {
        assert_eq!(terminal_split_position(700, 270), 430);
        assert_eq!(terminal_split_position(700, 10), 590);
        assert_eq!(terminal_split_position(700, 1_000), 120);
        assert_eq!(terminal_split_position(180, 270), 120);
    }
}

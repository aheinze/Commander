//! Keep enabled file panes inside their allocated split, including late allocations.
use super::*;

pub(super) fn install_pane_constraints(paned: &gtk::Paned) {
    let pending = Rc::new(Cell::new(false));
    // Width/height are getters on GtkWidget, not notifying GObject properties.
    // Paned's min/max-position properties track the actual allocated bounds.
    for property in ["min-position", "max-position", "position", "orientation"] {
        let pending = Rc::clone(&pending);
        paned.connect_notify_local(Some(property), move |paned, _| schedule(paned, &pending));
    }
    let map_pending = Rc::clone(&pending);
    paned.connect_map(move |paned| schedule(paned, &map_pending));
    for child in [paned.start_child(), paned.end_child()]
        .into_iter()
        .flatten()
    {
        let weak = paned.downgrade();
        let pending = Rc::clone(&pending);
        child.connect_visible_notify(move |_| {
            if let Some(paned) = weak.upgrade() {
                schedule(&paned, &pending);
            }
        });
    }
}

fn schedule(paned: &gtk::Paned, pending: &Rc<Cell<bool>>) {
    if pending.replace(true) {
        return;
    }
    let weak = paned.downgrade();
    let pending = Rc::clone(pending);
    // Run after allocation instead of queueing a resize from inside allocation.
    // Coalescing also avoids chasing intermediate inspector/revealer positions.
    glib::idle_add_local_once(move || {
        pending.set(false);
        let Some(paned) = weak.upgrade() else { return };
        if !paned.is_mapped()
            || !paned.start_child().is_some_and(|child| child.is_visible())
            || !paned.end_child().is_some_and(|child| child.is_visible())
        {
            return;
        }
        let minimum = paned.min_position();
        let maximum = paned.max_position();
        let available = maximum.saturating_sub(minimum);
        if available <= 0 {
            return;
        }
        let pane_minimum = if paned.orientation() == gtk::Orientation::Horizontal {
            240
        } else {
            120
        };
        let margin = pane_minimum.min(available / 2);
        let position = paned.position().clamp(minimum + margin, maximum - margin);
        if position != paned.position() {
            paned.set_position(position);
        }
    });
}

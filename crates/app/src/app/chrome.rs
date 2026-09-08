//! Shared window and dialog close controls.

use super::*;

pub(super) fn close_button() -> gtk::Button {
    let button = gtk::Button::from_icon_name("commander-x-symbolic");
    button.add_css_class("window-action");
    button.add_css_class("window-close");
    button.set_tooltip_text(Some("Close"));
    button.update_property(&[gtk::accessible::Property::Label("Close")]);
    button.set_size_request(24, 24);
    button.set_valign(gtk::Align::Center);
    button
}

pub(super) fn dialog_close_button(dialog: &adw::Dialog) -> gtk::Button {
    let button = close_button();
    dialog
        .bind_property("can-close", &button, "sensitive")
        .sync_create()
        .build();
    let dialog = dialog.downgrade();
    button.connect_clicked(move |_| {
        if let Some(dialog) = dialog.upgrade() {
            dialog.close();
        }
    });
    button
}

pub(super) fn dialog_header(dialog: &adw::Dialog) -> adw::HeaderBar {
    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.pack_end(&dialog_close_button(dialog));
    header
}

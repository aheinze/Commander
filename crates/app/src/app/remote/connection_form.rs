use super::connection::{ConnectionFields, PROTOCOLS, RemoteConnection};
use super::*;

#[derive(Clone)]
pub(super) struct ConnectionForm(Rc<FormWidgets>);

struct FormWidgets {
    root: gtk::Box,
    name: gtk::Entry,
    protocol: gtk::DropDown,
    host: gtk::Entry,
    port: gtk::Entry,
    folder: gtk::Entry,
    folder_label: gtk::Label,
    username: gtk::Entry,
    password: gtk::PasswordEntry,
    domain: gtk::Entry,
    domain_row: gtk::Box,
    anonymous: gtk::CheckButton,
    anonymous_row: gtk::Box,
    auth: gtk::Box,
    named: gtk::Box,
    remember: gtk::DropDown,
    hint: gtk::Label,
    status: notifications::Feedback,
    connect: glib::WeakRef<gtk::Button>,
    save_only: RefCell<Option<glib::WeakRef<gtk::Button>>>,
    loading: Cell<bool>,
}

impl ConnectionForm {
    pub fn new(connect: &gtk::Button) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("dialog-body");
        root.add_css_class("remote-form");
        let name = entry("Optional · shown in the sidebar");
        name.set_tooltip_text(Some("Leave blank to use the server address"));
        root.append(&field_row("Name", &name).0);
        let protocol =
            gtk::DropDown::from_strings(&PROTOCOLS.iter().map(|p| p.label).collect::<Vec<_>>());
        root.append(&field_row("Connection", &protocol).0);
        let host = entry("Hostname or paste a server URL");
        let port = entry("22");
        port.set_width_chars(5);
        port.set_max_width_chars(5);
        port.set_input_purpose(gtk::InputPurpose::Digits);
        port.set_hexpand(false);
        port.update_property(&[gtk::accessible::Property::Label("Port")]);
        port.set_tooltip_text(Some("Leave blank to use the default port"));
        let server = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        host.set_hexpand(true);
        server.append(&host);
        let port_label = gtk::Label::new(Some("Port"));
        port_label.add_css_class("remote-field-label");
        server.append(&port_label);
        server.append(&port);
        root.append(&field_row("Server", &server).0);
        host.update_property(&[gtk::accessible::Property::Label("Server")]);
        let folder = entry("/ · server root");
        let (folder_row, folder_label) = field_row("Folder", &folder);
        root.append(&folder_row);

        let auth = gtk::Box::new(gtk::Orientation::Vertical, 8);
        auth.add_css_class("remote-auth");
        let anonymous = gtk::CheckButton::with_label("Anonymous login");
        let (anonymous_row, _) = field_row("", &anonymous);
        auth.append(&anonymous_row);
        let named = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let username = entry("Username");
        named.append(&field_row("Username", &username).0);
        let password = gtk::PasswordEntry::new();
        password.set_show_peek_icon(true);
        password.set_activates_default(true);
        password.set_placeholder_text(Some("Optional · ask when connecting"));
        named.append(&field_row("Password", &password).0);
        let domain = entry("Optional workgroup or domain");
        let (domain_row, _) = field_row("Domain", &domain);
        named.append(&domain_row);
        let remember =
            gtk::DropDown::from_strings(&["Do not save", "Until logout", "Save in keyring"]);
        named.append(&field_row("Password storage", &remember).0);
        auth.append(&named);
        root.append(&auth);
        let hint = caption();
        root.append(&hint);
        let status = notifications::Feedback::default();
        let form = Self(Rc::new(FormWidgets {
            root,
            name,
            protocol,
            host,
            port,
            folder,
            folder_label,
            username,
            password,
            domain,
            domain_row,
            anonymous,
            anonymous_row,
            auth,
            named,
            remember,
            hint,
            status,
            connect: connect.downgrade(),
            save_only: RefCell::new(None),
            loading: Cell::new(false),
        }));
        form.connect_signals();
        form.update();
        form
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.0.root
    }

    pub fn clear_password(&self) {
        self.0.password.set_text("");
    }

    pub fn name(&self) -> String {
        self.0.name.text().trim().to_owned()
    }

    pub fn set_name(&self, name: &str) {
        self.0.name.set_text(name);
    }

    pub fn set_save_button(&self, button: &gtk::Button) {
        self.0.save_only.replace(Some(button.downgrade()));
        self.update();
    }

    fn set_ready(&self, ready: bool) {
        if let Some(button) = self.0.connect.upgrade() {
            button.set_sensitive(ready);
        }
        if let Some(button) = self
            .0
            .save_only
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        {
            let has_password = !self.0.password.text().is_empty();
            button.set_sensitive(ready && !has_password);
            button.set_tooltip_text(has_password.then_some(
                "Choose Save and connect to use the password and its storage preference",
            ));
        }
    }

    pub fn focus(&self) {
        self.0.host.grab_focus();
    }

    fn connect_signals(&self) {
        let weak = Rc::downgrade(&self.0);
        self.0.protocol.connect_selected_notify(move |_| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.loading.get() {
                return;
            }
            w.loading.set(true);
            w.port.set_text("");
            w.password.set_text("");
            w.domain.set_text("");
            w.anonymous.set_active(false);
            w.loading.set(false);
            Self(w).update();
        });
        let weak = Rc::downgrade(&self.0);
        self.0.port.connect_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                if w.loading.get() {
                    return;
                }
                w.password.set_text("");
                Self(w).update();
            }
        });
        let weak = Rc::downgrade(&self.0);
        self.0.host.connect_changed(move |entry| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if w.loading.get() {
                return;
            }
            let form = Self(w);
            // A complete pasted URL fills the same fields as a saved location.
            if entry.text().contains("://") {
                form.load_uri(&entry.text());
            } else {
                form.0.password.set_text("");
                form.update();
            }
        });
        for editable in [
            self.0.folder.clone().upcast::<gtk::Editable>(),
            self.0.username.clone().upcast(),
            self.0.password.clone().upcast(),
            self.0.domain.clone().upcast(),
        ] {
            let weak = Rc::downgrade(&self.0);
            editable.connect_changed(move |_| {
                if let Some(w) = weak.upgrade() {
                    Self(w).update();
                }
            });
        }
        let weak = Rc::downgrade(&self.0);
        self.0.anonymous.connect_toggled(move |_| {
            if let Some(w) = weak.upgrade() {
                Self(w).update();
            }
        });
    }

    fn fields(&self) -> ConnectionFields {
        let w = &self.0;
        ConnectionFields {
            protocol: w.protocol.selected() as usize,
            host: w.host.text().to_string(),
            port: w.port.text().to_string(),
            folder: w.folder.text().to_string(),
            username: w.username.text().to_string(),
            password: w.password.text().to_string(),
            domain: w.domain.text().to_string(),
            anonymous: w.anonymous.is_active(),
            save: match w.remember.selected() {
                1 => gio::PasswordSave::ForSession,
                2 => gio::PasswordSave::Permanently,
                _ => gio::PasswordSave::Never,
            },
        }
    }

    pub fn connection(&self) -> Result<RemoteConnection, &'static str> {
        let fields = self.fields();
        let connection = fields.connection()?;
        if matches!(PROTOCOLS[fields.protocol].scheme, "ftp" | "ftps")
            && !fields.anonymous
            && fields.username.trim().is_empty()
        {
            return Err("Enter a username or choose Anonymous login.");
        }
        Ok(connection)
    }

    pub fn load_uri(&self, uri: &str) {
        let fields = match ConnectionFields::parse(uri) {
            Ok(fields) => fields,
            Err(error) => {
                self.0.status.validate(Some(error), &self.0.host);
                self.set_ready(false);
                return;
            }
        };
        let w = &self.0;
        w.loading.set(true);
        w.protocol.set_selected(fields.protocol as u32);
        w.host.set_text(&fields.host);
        w.port.set_text(&fields.port);
        w.folder.set_text(&fields.folder);
        w.username.set_text(&fields.username);
        w.password.set_text(&fields.password);
        w.domain.set_text(&fields.domain);
        w.anonymous.set_active(fields.anonymous);
        w.remember.set_selected(0);
        w.loading.set(false);
        self.update();
    }

    fn update(&self) {
        let w = &self.0;
        if w.loading.get() {
            return;
        }
        let protocol = &PROTOCOLS[w.protocol.selected() as usize];
        w.port
            .set_placeholder_text(Some(&protocol.port.to_string()));
        w.port
            .set_tooltip_text(Some(&format!("Port · default {}", protocol.port)));
        let smb = protocol.scheme == "smb";
        w.folder_label
            .set_label(if smb { "Share / folder" } else { "Folder" });
        w.folder.set_placeholder_text(Some(if smb {
            "/share/optional/folder"
        } else {
            "/ · server root"
        }));
        w.auth.set_visible(protocol.scheme != "nfs");
        w.domain_row.set_visible(smb);
        let guest_supported = matches!(protocol.scheme, "ftp" | "ftps" | "smb" | "afp");
        w.anonymous.set_visible(guest_supported);
        w.anonymous_row.set_visible(guest_supported);
        w.anonymous
            .set_label(Some(if smb || protocol.scheme == "afp" {
                "Connect as guest"
            } else {
                "Anonymous login"
            }));
        w.named.set_sensitive(!w.anonymous.is_active());
        w.remember.set_sensitive(!w.password.text().is_empty());
        w.hint.set_label(match protocol.scheme {
            "sftp" => "Leave the password blank to use an SSH key, a saved login, or a sign-in prompt.",
            "ftp" => "FTP sends login details without encryption. Choose FTP · TLS or SFTP for encrypted access.",
            "nfs" => "Access uses the server’s export permissions and your local user identity.",
            _ => "Leave the password blank to use a saved login or sign in when connecting.",
        });
        match self.connection() {
            Ok(connection) => {
                w.status.validate(None, &w.host);
                w.host.set_tooltip_text(Some(&connection.uri));
                self.set_ready(true);
            }
            Err(error) => {
                w.status
                    .validate((!w.host.text().is_empty()).then_some(error), &w.host);
                self.set_ready(false);
            }
        }
    }
}

fn entry(placeholder: &str) -> gtk::Entry {
    gtk::Entry::builder()
        .placeholder_text(placeholder)
        .activates_default(true)
        .build()
}

fn caption() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.add_css_class("remote-form-caption");
    label
}

fn field_row(title: &str, widget: &impl IsA<gtk::Widget>) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    label.set_width_chars(16);
    label.set_mnemonic_widget(Some(widget));
    label.add_css_class("remote-field-label");
    widget.set_hexpand(true);
    if !title.is_empty() {
        widget
            .as_ref()
            .update_property(&[gtk::accessible::Property::Label(title)]);
    }
    row.append(&label);
    row.append(widget);
    (row, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a GTK display; run alone with --ignored --test-threads=1"]
    fn gtk_connection_form_handles_protocols_credentials_and_saved_locations() {
        adw::init().unwrap();
        crate::icons::install(&gdk::Display::default().unwrap());
        install_styles(AppearanceMode::Dark);
        let connect = gtk::Button::with_label("Connect");
        connect.add_css_class("suggested-action");
        let form = ConnectionForm::new(&connect);
        assert!(!connect.is_sensitive());
        let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        shell.append(form.widget());
        let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
        list.add_css_class("remote-list");
        list.append(&section_header("On your network"));
        let row = server_row(
            RowKind::Server,
            "Studio NAS",
            Some("SMB"),
            "smb://nas.local/Projects",
            &form,
        );
        list.append(&row);
        list.append(&section_header("Recent"));
        list.append(&server_row(
            RowKind::Recent,
            "Files",
            Some("SFTP"),
            "sftp://alex@files.local/Projects",
            &form,
        ));
        shell.append(&list);
        let actions = dialog_actions();
        actions.append(&gtk::Button::with_label("Cancel"));
        actions.append(&connect);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&shell)
            .build();
        root.append(&scroll);
        root.append(&actions);
        let (dialog, view) = utility_dialog("Connect to Server", 560, 660, "remote-dialog");
        view.set_content(Some(&root));
        // Give the standalone snapshot the same background as its dialog sheet.
        view.add_css_class("remote-dialog-preview");
        let provider = gtk::CssProvider::new();
        provider.load_from_string(".remote-dialog-preview { background: @carelo_toolbar; }");
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().unwrap(),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let window = adw::Window::builder()
            .default_width(800)
            .default_height(850)
            .build();
        window.present();
        dialog.present(Some(&window));

        form.load_uri("ftp://alex@files.local:2121/Projects");
        form.0.password.set_text("test-password");
        assert!(connect.is_sensitive());
        assert_eq!(
            form.connection()
                .unwrap()
                .credentials
                .unwrap()
                .password
                .as_deref(),
            Some("test-password")
        );
        assert!(!form.0.status.text().contains("test-password"));
        assert!(form.0.anonymous.is_visible());
        assert!(!form.0.domain_row.is_visible());
        snapshot(&view, "ftp");
        form.0.username.set_text("");
        assert!(!connect.is_sensitive());
        form.0.anonymous.set_active(true);
        assert!(connect.is_sensitive());
        assert!(!form.0.named.is_sensitive());
        assert!(
            form.connection()
                .unwrap()
                .credentials
                .unwrap()
                .password
                .is_none()
        );
        form.0.port.set_text("65536");
        assert!(!connect.is_sensitive());
        form.0.port.set_text("21");
        assert!(connect.is_sensitive());
        form.0.host.set_text("another.local");
        assert!(form.0.password.text().is_empty());

        row.emit_clicked();
        assert_eq!(form.0.host.text(), "nas.local");
        assert_eq!(form.0.folder.text(), "/Projects");
        assert!(!form.0.anonymous.is_active());
        assert!(form.0.domain_row.is_visible());
        form.0.username.set_text("alex");
        form.0.domain.set_text("STUDIO");
        snapshot(&view, "smb");
        form.0
            .host
            .set_text("sftp://alex:paste%40password@[::1]:2222/Work%20files");
        assert_eq!(form.0.host.text(), "::1");
        assert_eq!(form.0.folder.text(), "/Work files");
        assert_eq!(form.0.password.text(), "paste@password");
        assert!(!form.0.anonymous.is_visible());
        assert!(!form.0.domain_row.is_visible());
        assert!(connect.is_sensitive());
        form.clear_password();
        snapshot(&view, "sftp");
        install_styles(AppearanceMode::Light);
        snapshot(&view, "sftp-light");
        form.load_uri("nfs://nas.local/export");
        assert!(!form.0.auth.is_visible());
        assert!(connect.is_sensitive());
        assert!(form.connection().unwrap().credentials.is_none());
        // Submitted credentials are consumed once. A repeated challenge must
        // fall back to the prompt (aborted here because there is no app window).
        let operation = mount_operation_with_credentials(
            RemoteConnection::parse("ftp://alex:test-password@files.local")
                .unwrap()
                .credentials,
        );
        let replies = Rc::new(RefCell::new(Vec::new()));
        let seen = replies.clone();
        operation.connect_reply(move |_, result| seen.borrow_mut().push(result));
        let flags = gio::AskPasswordFlags::NEED_USERNAME | gio::AskPasswordFlags::NEED_PASSWORD;
        for _ in 0..2 {
            operation.emit_by_name::<()>("ask-password", &[&"Password", &"alex", &"", &flags]);
        }
        assert_eq!(
            *replies.borrow(),
            vec![
                gio::MountOperationResult::Handled,
                gio::MountOperationResult::Aborted
            ]
        );
        dialog.close();
        window.close();
    }

    fn snapshot(widget: &impl IsA<gtk::Widget>, name: &str) {
        let paintable = gtk::WidgetPaintable::new(Some(widget));
        let context = glib::MainContext::default();
        let until = Instant::now() + Duration::from_millis(800);
        while Instant::now() < until {
            while context.pending() {
                context.iteration(false);
            }
            thread::sleep(Duration::from_millis(5));
        }
        let Some(directory) = std::env::var_os("COMMANDER_REMOTE_SNAPSHOT_DIR") else {
            return;
        };
        let widget = widget.as_ref();
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            f64::from(widget.width()),
            f64::from(widget.height()),
        );
        let node = snapshot.to_node().unwrap_or_else(|| {
            panic!(
                "{name} snapshot empty at {} × {}; visible={}, mapped={}, realized={}",
                widget.width(),
                widget.height(),
                widget.is_visible(),
                widget.is_mapped(),
                widget.is_realized()
            )
        });
        widget
            .native()
            .unwrap()
            .renderer()
            .unwrap()
            .render_texture(&node, None)
            .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
            .unwrap();
    }
}

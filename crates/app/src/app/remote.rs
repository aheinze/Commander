//! The Connect to Server sheet: connection details, servers advertised on the
//! local network (DNS-SD via Avahi), the shares those servers expose, and
//! recently used locations.

mod connection;
mod connection_form;

pub(crate) use connection::RemoteConnection;
use connection_form::ConnectionForm;
use std::process::Command;

use super::dialogs::{dialog_actions, form_row, utility_dialog};
use super::*;

/// A file service advertised on the local network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiscoveredServer {
    pub(super) name: String,
    pub(super) uri: String,
    pub(super) protocol: &'static str,
}

impl DiscoveredServer {
    fn is_smb(&self) -> bool {
        self.protocol == "SMB"
    }
}

/// DNS-SD service types that map onto a GVfs backend, with the label shown to the user.
const SERVICE_TYPES: &[(&str, &str)] = &[
    ("_smb._tcp", "SMB"),
    ("_sftp-ssh._tcp", "SFTP"),
    ("_ftp._tcp", "FTP"),
    ("_webdav._tcp", "WebDAV"),
    ("_webdavs._tcp", "WebDAV"),
    ("_afpovertcp._tcp", "AFP"),
    ("_nfs._tcp", "NFS"),
];

/// Browses the local network once. Runs `avahi-browse` and returns when its
/// cache is exhausted, so it belongs on a worker thread.
pub(super) fn discover_servers() -> Vec<DiscoveredServer> {
    let output = Command::new("avahi-browse")
        .args([
            "--all",
            "--resolve",
            "--terminate",
            "--parsable",
            "--no-db-lookup",
        ])
        .output();
    match output {
        Ok(output) => parse_avahi(&String::from_utf8_lossy(&output.stdout)),
        Err(error) => {
            tracing::info!(%error, "avahi-browse is not available; skipping network discovery");
            Vec::new()
        }
    }
}

/// Parses `avahi-browse --parsable` output. Resolved records look like
/// `=;iface;IPv4;name;type;domain;host;address;port;"txt" "txt"`.
pub(super) fn parse_avahi(output: &str) -> Vec<DiscoveredServer> {
    let mut servers: Vec<DiscoveredServer> = Vec::new();
    // IPv4 records first so a host advertised over both families keeps one entry.
    let mut records: Vec<Vec<&str>> = output
        .lines()
        .filter(|line| line.starts_with("=;"))
        .map(|line| line.splitn(10, ';').collect::<Vec<_>>())
        .filter(|fields| fields.len() >= 9)
        .collect();
    records.sort_by_key(|fields| fields[2] != "IPv4");
    for fields in records {
        let Some((_, protocol)) = SERVICE_TYPES
            .iter()
            .find(|(service, _)| *service == fields[4])
        else {
            continue;
        };
        let host = fields[6].trim_end_matches('.');
        if host.is_empty() {
            continue;
        }
        let port: u16 = fields[8].parse().unwrap_or(0);
        let txt = fields.get(9).copied().unwrap_or_default();
        let path = txt_value(txt, "path");
        let uri = service_uri(fields[4], host, port, path.as_deref());
        if servers.iter().any(|server| server.uri == uri) {
            continue;
        }
        servers.push(DiscoveredServer {
            name: unescape_avahi(fields[3]),
            uri,
            protocol,
        });
    }
    servers.sort_by_key(|server| server.name.to_lowercase());
    servers
}

fn service_uri(service: &str, host: &str, port: u16, path: Option<&str>) -> String {
    let authority = |default_port: u16| {
        if port == 0 || port == default_port {
            host.to_owned()
        } else {
            format!("{host}:{port}")
        }
    };
    let with_path = |root: String| match path {
        // Some services (FTP on routers) advertise the complete address.
        Some(path) if path.contains("://") => path.to_owned(),
        Some(path) => format!("{root}/{}", path.trim_start_matches('/')),
        None => format!("{root}/"),
    };
    match service {
        "_smb._tcp" => format!("smb://{}/", authority(445)),
        "_sftp-ssh._tcp" => format!("sftp://{}/", authority(22)),
        "_ftp._tcp" => with_path(format!("ftp://{}", authority(21))),
        "_webdav._tcp" => with_path(format!("dav://{}", authority(80))),
        "_webdavs._tcp" => with_path(format!("davs://{}", authority(443))),
        "_afpovertcp._tcp" => format!("afp://{}/", authority(548)),
        "_nfs._tcp" => with_path(format!("nfs://{}", authority(2049))),
        _ => format!("{host}/"),
    }
}

/// Looks up `key=value` in the quoted TXT record list.
fn txt_value(txt: &str, key: &str) -> Option<String> {
    txt.split('"')
        .filter(|entry| !entry.trim().is_empty())
        .find_map(|entry| {
            entry
                .strip_prefix(key)?
                .strip_prefix('=')
                .map(str::to_owned)
        })
        .filter(|value| !value.is_empty())
}

/// Reverses avahi's escaping (`\032` for a space, `\.`, `\\`).
fn unescape_avahi(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut chars = name.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        let digits: String = chars
            .clone()
            .take(3)
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.len() == 3 {
            for _ in 0..3 {
                chars.next();
            }
            if let Some(decoded) = digits.parse::<u32>().ok().and_then(char::from_u32) {
                result.push(decoded);
            }
        } else if let Some(next) = chars.next() {
            result.push(next);
        }
    }
    result
}

/// Which section of the list a row belongs to.
#[derive(Clone, Copy)]
enum RowKind {
    Server,
    Share,
    Recent,
}

/// Opens the Connect sheet. Editing fills the saved connection details and
/// replaces that location when submitted.
pub(super) fn show_remote_dialog(
    recent: Vec<String>,
    editing: Option<String>,
    sender: &ComponentSender<AppModel>,
) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let (dialog, view) = utility_dialog("Connect to Server", 560, 660, "remote-dialog");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    let form = ConnectionForm::new(&connect);
    if let Some(uri) = &editing {
        form.load_uri(uri);
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(form.widget());

    let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    list.add_css_class("remote-list");
    let network_header = section_header("On your network");
    let spinner = gtk::Spinner::new();
    spinner.set_spinning(true);
    spinner.add_css_class("remote-spinner");
    network_header.append(&spinner);
    let network_status = gtk::Label::new(Some("Searching…"));
    network_status.add_css_class("remote-section-status");
    network_header.append(&network_status);
    list.append(&network_header);
    let network_rows = gtk::Box::new(gtk::Orientation::Vertical, 1);
    list.append(&network_rows);
    if !recent.is_empty() {
        list.append(&section_header("Recent"));
        for uri in &recent {
            let title = uri
                .split_once("://")
                .map_or(uri.as_str(), |(_, rest)| rest)
                .trim_end_matches('/');
            list.append(&server_row(
                RowKind::Recent,
                title,
                Some(scheme_label(uri)),
                uri,
                &form,
            ));
        }
    }
    content.append(&list);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&content)
        .build();
    root.append(&scrolled);

    let actions = dialog_actions();
    let cancel = gtk::Button::with_label("Cancel");
    actions.append(&cancel);
    actions.append(&connect);
    root.append(&actions);
    view.set_content(Some(&root));
    dialog.set_default_widget(Some(&connect));
    {
        let dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    {
        let input = sender.input_sender().clone();
        let form = form.clone();
        let dialog = dialog.downgrade();
        let editing = editing.clone();
        connect.connect_clicked(move |_| {
            let Ok(connection) = form.connection() else {
                return;
            };
            let _ = input.send(AppMsg::ConnectRemoteWithOptions {
                connection,
                replacing: editing.clone(),
            });
            form.clear_password();
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }

    // Discovery outlives nothing: closing the sheet cancels share browsing and
    // drops whatever the worker thread still reports.
    let cancellable = gio::Cancellable::new();
    {
        let cancellable = cancellable.clone();
        let form = form.clone();
        dialog.connect_closed(move |_| {
            cancellable.cancel();
            form.clear_password();
        });
    }
    {
        let form = form.clone();
        let cancellable = cancellable.clone();
        glib::spawn_future_local(async move {
            let servers = gio::spawn_blocking(discover_servers)
                .await
                .unwrap_or_default();
            spinner.set_spinning(false);
            spinner.set_visible(false);
            if cancellable.is_cancelled() {
                return;
            }
            if servers.is_empty() {
                network_status.set_label("Nothing found");
                let empty = gtk::Label::new(Some(
                    "No shared folders are advertised on this network. Servers announcing SMB, SFTP, FTP, WebDAV, AFP, or NFS over Bonjour/Avahi appear here.",
                ));
                empty.set_wrap(true);
                empty.set_xalign(0.0);
                empty.add_css_class("remote-empty");
                network_rows.append(&empty);
                return;
            }
            network_status.set_label(&format!(
                "{} server{}",
                servers.len(),
                if servers.len() == 1 { "" } else { "s" }
            ));
            for server in servers {
                let row = server_row(
                    RowKind::Server,
                    &server.name,
                    Some(server.protocol),
                    &server.uri,
                    &form,
                );
                network_rows.append(&row);
                if server.is_smb() {
                    let shares = gtk::Box::new(gtk::Orientation::Vertical, 1);
                    network_rows.append(&shares);
                    let share_cancel = gio::Cancellable::new();
                    let cancel_on_close = share_cancel.clone();
                    cancellable.connect_cancelled(move |_| cancel_on_close.cancel());
                    list_smb_shares(server, shares, form.clone(), share_cancel);
                }
            }
        });
    }
    dialog.present(Some(&window));
    form.focus();
}

/// Explains a failed connection and offers the ways out of it: fix the
/// address, or retry (with a fresh credential prompt when it looks like the
/// password was the problem).
pub(super) fn show_remote_error(uri: String, error: String, sender: &ComponentSender<AppModel>) {
    let Some(window) = relm4::main_application().active_window() else {
        return;
    };
    let auth_related = looks_like_auth_failure(&error);
    let dialog = AlertSheet::new(Some("Could not connect"), None);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.add_css_class("remote-error");
    let address = gtk::Label::new(Some(&uri));
    address.set_xalign(0.0);
    address.set_halign(gtk::Align::Start);
    address.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    address.set_selectable(true);
    address.set_can_focus(false);
    address.add_css_class("remote-error-address");
    content.append(&address);
    let callout = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    callout.add_css_class("remote-error-callout");
    let icon = gtk::Image::from_icon_name(if auth_related {
        "commander-key-round-symbolic"
    } else {
        "commander-unplug-symbolic"
    });
    icon.set_pixel_size(16);
    icon.set_valign(gtk::Align::Start);
    icon.add_css_class("remote-error-icon");
    callout.append(&icon);
    let message = gtk::Label::new(Some(error.trim_end_matches('.')));
    message.set_xalign(0.0);
    message.set_wrap(true);
    message.set_hexpand(true);
    message.add_css_class("remote-error-message");
    callout.append(&message);
    content.append(&callout);
    let hint = gtk::Label::new(Some(if auth_related {
        "Forgetting the saved password asks for a new one on the next attempt. To change the username or password, edit the connection."
    } else {
        "Check that the server is reachable, then try again or edit the connection."
    }));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");
    content.append(&hint);
    dialog.set_extra_child(Some(&content));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("edit", "Edit Address…");
    if auth_related {
        dialog.add_response("forget", "Forget Password & Retry");
        dialog.set_default_response(Some("forget"));
        dialog.set_response_appearance("forget", adw::ResponseAppearance::Suggested);
    } else {
        dialog.add_response("retry", "Try Again");
        dialog.set_default_response(Some("retry"));
        dialog.set_response_appearance("retry", adw::ResponseAppearance::Suggested);
    }
    dialog.set_close_response("cancel");
    let input = sender.input_sender().clone();
    dialog.connect_response(None, move |_, response| {
        let _ = match response {
            "retry" => input.send(AppMsg::ConnectRemote(uri.clone())),
            "forget" => input.send(AppMsg::ForgetRemotePassword(uri.clone())),
            "edit" => input.send(AppMsg::EditRemote(uri.clone())),
            _ => Ok(()),
        };
    });
    dialog.present(Some(&window));
}

pub(super) fn looks_like_auth_failure(error: &str) -> bool {
    let lower = error.to_lowercase();
    [
        "permission denied",
        "access denied",
        "authentication",
        "password",
        "login",
        "not authorized",
        "unauthorized",
        "credentials",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Removes the passwords GVfs stored in the keyring for the address's server.
/// Returns how many entries were cleared. GVfs records the server exactly as
/// typed, so a `.local` host is cleared under both spellings.
pub(super) fn forget_remote_password(uri: &str) -> Result<usize, String> {
    let fields = connection::ConnectionFields::parse(uri).map_err(str::to_owned)?;
    let scheme = connection::PROTOCOLS[fields.protocol].scheme;
    let host = fields.host;
    let mut servers = vec![host.to_owned()];
    if let Some(short) = host.strip_suffix(".local") {
        servers.push(short.to_owned());
    }
    let mut cleared = 0;
    for server in servers {
        let matches = Command::new("secret-tool")
            .args(["search", "--all", "protocol", scheme, "server", &server])
            .output()
            .map_err(|error| format!("secret-tool is not available: {error}"))?;
        cleared += String::from_utf8_lossy(&matches.stdout)
            .lines()
            .filter(|line| line.starts_with("[/org/freedesktop/secrets/"))
            .count();
        let status = Command::new("secret-tool")
            .args(["clear", "protocol", scheme, "server", &server])
            .status()
            .map_err(|error| format!("secret-tool is not available: {error}"))?;
        if !status.success() {
            return Err(format!("Could not clear the saved password for {server}"));
        }
    }
    Ok(cleared)
}

/// Mounts an SMB host anonymously and lists its shares under the host row.
/// Hosts that insist on credentials simply keep their single row; connecting
/// to them still goes through the normal password prompt.
fn list_smb_shares(
    server: DiscoveredServer,
    shares: gtk::Box,
    form: ConnectionForm,
    cancellable: gio::Cancellable,
) {
    let file = gio::File::for_uri(&server.uri);
    let operation = gio::MountOperation::new();
    operation.set_anonymous(true);
    let attempted = Rc::new(Cell::new(false));
    operation.connect_ask_password(move |operation, _, _, _, flags| {
        operation.stop_signal_emission_by_name("ask-password");
        if flags.contains(gio::AskPasswordFlags::ANONYMOUS_SUPPORTED) && !attempted.replace(true) {
            operation.set_anonymous(true);
            operation.reply(gio::MountOperationResult::Handled);
        } else {
            operation.reply(gio::MountOperationResult::Aborted);
        }
    });
    // Do not let a silent host hold the sheet's discovery open forever.
    {
        let cancellable = cancellable.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(10), move || {
            cancellable.cancel();
        });
    }
    let enumerate_cancellable = cancellable.clone();
    let mounted = file.clone();
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&operation),
        Some(&cancellable),
        move |result| {
            let already_mounted = matches!(
                &result,
                Err(error) if error.matches(gio::IOErrorEnum::AlreadyMounted)
            );
            if result.is_err() && !already_mounted {
                return;
            }
            let uri = server.uri.clone();
            let next_cancellable = enumerate_cancellable.clone();
            mounted.enumerate_children_async(
                "standard::name,standard::display-name,standard::type",
                gio::FileQueryInfoFlags::NONE,
                glib::Priority::DEFAULT,
                Some(&enumerate_cancellable),
                move |enumerator| {
                    let Ok(enumerator) = enumerator else {
                        return;
                    };
                    enumerator.next_files_async(
                        128,
                        glib::Priority::DEFAULT,
                        Some(&next_cancellable),
                        move |infos| {
                            let Ok(infos) = infos else {
                                return;
                            };
                            for info in infos
                                .iter()
                                .filter(|info| info.file_type() == gio::FileType::Directory)
                            {
                                let name = info.name().to_string_lossy().into_owned();
                                if name.ends_with('$') {
                                    continue;
                                }
                                let share_uri = gio::File::for_uri(&uri).child(&name).uri();
                                let row = server_row(
                                    RowKind::Share,
                                    &info.display_name(),
                                    None,
                                    &share_uri,
                                    &form,
                                );
                                shares.append(&row);
                            }
                        },
                    );
                },
            );
        },
    );
}

fn section_header(title: &str) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("remote-section");
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    label.add_css_class("dialog-eyebrow");
    header.append(&label);
    header
}

/// One selectable location. Clicking fills the connection fields; Connect or
/// Enter then opens it, so the user always sees what is about to be mounted.
fn server_row(
    kind: RowKind,
    title: &str,
    badge: Option<&str>,
    uri: &str,
    form: &ConnectionForm,
) -> gtk::Button {
    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("remote-row");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::from_icon_name(match kind {
        RowKind::Server => "commander-server-symbolic",
        RowKind::Share => "commander-cloud-symbolic",
        RowKind::Recent => "commander-rotate-ccw-clock-symbolic",
    });
    icon.set_pixel_size(16);
    icon.add_css_class("remote-row-icon");
    content.append(&icon);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 1);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("remote-row-title");
    labels.append(&name);
    let detail = gtk::Label::new(Some(uri));
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    detail.add_css_class("remote-row-subtitle");
    labels.append(&detail);
    content.append(&labels);
    if let Some(badge) = badge {
        let badge = gtk::Label::new(Some(badge));
        badge.add_css_class("remote-badge");
        badge.set_valign(gtk::Align::Center);
        content.append(&badge);
    }
    if matches!(kind, RowKind::Share) {
        row.add_css_class("remote-row-nested");
    }
    row.set_child(Some(&content));
    let uri = uri.to_owned();
    let form = form.clone();
    row.connect_clicked(move |_| {
        form.load_uri(&uri);
        form.focus();
    });
    row
}

fn scheme_label(uri: &str) -> &'static str {
    match uri.split("://").next().unwrap_or_default() {
        "smb" => "SMB",
        "sftp" => "SFTP",
        "ftp" => "FTP",
        "ftps" => "FTPS",
        "dav" | "davs" => "WebDAV",
        "afp" => "AFP",
        "nfs" => "NFS",
        "s3" => "S3",
        _ => "Remote",
    }
}

/// Mounts a connection and resolves the requested folder through GVfs/FUSE.
pub(super) async fn mount_connection(connection: RemoteConnection) -> Result<VPath, String> {
    let file = gio::File::for_uri(&connection.uri);
    let operation = mount_operation_with_credentials(connection.credentials);
    if let Err(error) = file
        .mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&operation))
        .await
        && !error.matches(gio::IOErrorEnum::AlreadyMounted)
    {
        return Err(error.to_string());
    }
    let info = file
        .query_info_future(
            "standard::type",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
        .map_err(|error| error.to_string())?;
    if info.file_type() != gio::FileType::Directory {
        return Err("The remote location is not a folder.".to_owned());
    }
    let path = file.path().or_else(|| {
        gio::VolumeMonitor::get()
            .mounts()
            .into_iter()
            .find_map(|mount| {
                let root = mount.root();
                let base = root.path()?;
                if root == file {
                    Some(base)
                } else {
                    root.relative_path(&file)
                        .map(|relative| base.join(relative))
                }
            })
    });
    path.map(VPath::from)
        .ok_or_else(|| "The remote mounted but did not expose a native filesystem path".to_owned())
}

/// Native credential and host-key sheets for mounts without a connection form.
pub(super) fn mount_operation() -> gio::MountOperation {
    mount_operation_with_credentials(None)
}

fn mount_operation_with_credentials(
    credentials: Option<connection::Credentials>,
) -> gio::MountOperation {
    let operation = gio::MountOperation::new();
    // GMountOperation's own class handlers reply "unhandled" from an idle, which
    // GVfs reports as a cancelled dialog. Stopping the emission after presenting
    // the sheet keeps the reply ours.
    let credentials = RefCell::new(credentials);
    operation.connect_ask_password(
        move |operation, message, default_user, default_domain, flags| {
            operation.stop_signal_emission_by_name("ask-password");
            let supplied = credentials.borrow_mut().take();
            if supplied
                .as_ref()
                .is_some_and(|login| login.reply_if_complete(operation, flags))
            {
                return;
            }
            let user = supplied
                .as_ref()
                .map(|login| login.username.as_str())
                .filter(|user| !user.is_empty())
                .unwrap_or(default_user);
            let domain = supplied
                .as_ref()
                .map(|login| login.domain.as_str())
                .filter(|domain| !domain.is_empty())
                .unwrap_or(default_domain);
            show_password_prompt(operation, message, user, domain, flags);
        },
    );
    // `ask-question` carries a string array, which the bindings do not wrap.
    operation.connect_local("ask-question", false, |values| {
        let operation = values
            .first()
            .and_then(|value| value.get::<gio::MountOperation>().ok());
        let message = values
            .get(1)
            .and_then(|value| value.get::<String>().ok())
            .unwrap_or_default();
        let choices = values
            .get(2)
            .and_then(|value| value.get::<Vec<String>>().ok())
            .unwrap_or_default();
        if let Some(operation) = operation {
            show_question_prompt(&operation, &message, &choices);
            operation.stop_signal_emission_by_name("ask-question");
        }
        None
    });
    operation
}

/// Splits a GVfs message into its first line (the point) and the rest.
fn split_message(message: &str) -> (String, Option<String>) {
    let message = message.trim();
    match message.split_once('\n') {
        Some((first, rest)) => {
            let rest = rest.trim();
            (
                first.trim_end_matches(':').to_owned(),
                (!rest.is_empty()).then(|| rest.to_owned()),
            )
        }
        None => (message.trim_end_matches(':').to_owned(), None),
    }
}

fn show_password_prompt(
    operation: &gio::MountOperation,
    message: &str,
    default_user: &str,
    default_domain: &str,
    flags: gio::AskPasswordFlags,
) {
    let Some(window) = relm4::main_application().active_window() else {
        operation.reply(gio::MountOperationResult::Aborted);
        return;
    };
    // GVfs prefixes a generic "Authentication Required" line; the second line
    // names the account and server, which is the useful part.
    let (line, rest) = split_message(message);
    let body = rest.unwrap_or(line);
    let dialog = AlertSheet::new(Some("Sign In"), Some(&body));
    let form = gtk::Box::new(gtk::Orientation::Vertical, 8);
    form.add_css_class("dialog-form");
    let username = gtk::Entry::new();
    username.set_text(default_user);
    username.set_activates_default(true);
    let domain = gtk::Entry::new();
    domain.set_text(default_domain);
    domain.set_activates_default(true);
    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    password.set_activates_default(true);
    let anonymous = gtk::CheckButton::with_label("Connect as guest, without a password");
    let remember = gtk::DropDown::from_strings(&[
        "Forget the password immediately",
        "Remember until you log out",
        "Remember forever",
    ]);
    remember.set_selected(1);
    let mut first_field: Option<gtk::Widget> = None;
    if flags.contains(gio::AskPasswordFlags::NEED_USERNAME) {
        form.append(&form_row("User name", &username));
        first_field.get_or_insert_with(|| username.clone().upcast());
    }
    if flags.contains(gio::AskPasswordFlags::NEED_DOMAIN) {
        form.append(&form_row("Domain", &domain));
        first_field.get_or_insert_with(|| domain.clone().upcast());
    }
    if flags.contains(gio::AskPasswordFlags::NEED_PASSWORD) {
        form.append(&form_row("Password", &password));
        first_field.get_or_insert_with(|| password.clone().upcast());
    }
    if flags.contains(gio::AskPasswordFlags::ANONYMOUS_SUPPORTED) {
        form.append(&anonymous);
        let username = username.clone();
        let domain = domain.clone();
        let password = password.clone();
        anonymous.connect_toggled(move |check| {
            let named = !check.is_active();
            username.set_sensitive(named);
            domain.set_sensitive(named);
            password.set_sensitive(named);
        });
    }
    if flags.contains(gio::AskPasswordFlags::SAVING_SUPPORTED)
        && flags.contains(gio::AskPasswordFlags::NEED_PASSWORD)
    {
        form.append(&form_row("Remember", &remember));
    }
    // The password field is what people came for; put the cursor there unless a
    // user name is still missing.
    let focus = if flags.contains(gio::AskPasswordFlags::NEED_PASSWORD) && !default_user.is_empty()
    {
        Some(password.clone().upcast::<gtk::Widget>())
    } else {
        first_field
    };
    dialog.set_extra_child(Some(&form));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("connect", "Connect");
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("connect"));
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    let operation = operation.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "connect" {
            operation.reply(gio::MountOperationResult::Aborted);
            return;
        }
        let guest = anonymous.is_active();
        operation.set_anonymous(guest);
        if !guest {
            if flags.contains(gio::AskPasswordFlags::NEED_USERNAME) {
                operation.set_username(Some(&username.text()));
            }
            if flags.contains(gio::AskPasswordFlags::NEED_DOMAIN) {
                operation.set_domain(Some(&domain.text()));
            }
            if flags.contains(gio::AskPasswordFlags::NEED_PASSWORD) {
                operation.set_password(Some(&password.text()));
            }
        }
        operation.set_password_save(match remember.selected() {
            0 => gio::PasswordSave::Never,
            2 => gio::PasswordSave::Permanently,
            _ => gio::PasswordSave::ForSession,
        });
        operation.reply(gio::MountOperationResult::Handled);
    });
    dialog.present(Some(&window));
    if let Some(field) = focus {
        field.grab_focus();
    }
}

fn show_question_prompt(operation: &gio::MountOperation, message: &str, choices: &[String]) {
    let Some(window) = relm4::main_application().active_window() else {
        operation.reply(gio::MountOperationResult::Aborted);
        return;
    };
    let (heading, body) = split_message(message);
    let dialog = AlertSheet::new(Some(&heading), body.as_deref());
    let ids: Vec<String> = (0..choices.len())
        .map(|index| format!("choice-{index}"))
        .collect();
    for (id, label) in ids.iter().zip(choices) {
        dialog.add_response(id, label);
    }
    // GVfs lists the cautious option last; make it the default and what Escape picks.
    if let Some(last) = ids.last() {
        dialog.set_default_response(Some(last));
    }
    let operation = operation.clone();
    dialog.connect_response(None, move |_, response| {
        match ids.iter().position(|id| id == response) {
            Some(index) => {
                operation.set_choice(i32::try_from(index).unwrap_or(0));
                operation.reply(gio::MountOperationResult::Handled);
            }
            None => operation.reply(gio::MountOperationResult::Aborted),
        }
    });
    dialog.present(Some(&window));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a disposable local FTP server and GVfs session"]
    fn ftp_mount_uses_supplied_login_and_opens_requested_folder() {
        let port = std::env::var("COMMANDER_FTP_TEST_PORT").expect("local FTP fixture port");
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    for userinfo in ["commander-test:test-password", "anonymous"] {
                        let connection = RemoteConnection::parse(&format!(
                            "ftp://{userinfo}@127.0.0.1:{port}/nested"
                        ))
                        .unwrap();
                        let file = gio::File::for_uri(&connection.uri);
                        let path = mount_connection(connection).await.unwrap();
                        assert_eq!(
                            std::fs::read_to_string(path.as_path().join("proof.txt")).unwrap(),
                            "FTP connection verified\n"
                        );
                        let mount = file.find_enclosing_mount(gio::Cancellable::NONE).unwrap();
                        mount
                            .unmount_with_operation_future(
                                gio::MountUnmountFlags::FORCE,
                                gio::MountOperation::NONE,
                            )
                            .await
                            .unwrap();
                    }
                })
            })
            .unwrap();
    }

    #[test]
    fn parses_resolved_avahi_records_into_uris() {
        let output = "\
+;wlp3s0;IPv4;AG-LOCAL-CLOUD;_smb._tcp;local
=;wlp3s0;IPv6;AG-LOCAL-CLOUD;_smb._tcp;local;ag-local-cloud.local;fe80::1;445;
=;wlp3s0;IPv4;AG-LOCAL-CLOUD;_smb._tcp;local;ag-local-cloud.local;192.168.1.2;445;
=;wlp3s0;IPv4;fritz-box;_ftp._tcp;local;fritz.box;192.168.1.1;21;\"path=ftp://fritz.box/\"
=;wlp3s0;IPv4;192-168-178-1;_ftp._tcp;local;fritz.box;192.168.1.1;21;\"path=ftp://fritz.box/\"
=;wlp3s0;IPv4;Backup\\032Box;_sftp-ssh._tcp;local;backup.local;192.168.1.3;2222;
=;wlp3s0;IPv4;Cloud;_webdavs._tcp;local;cloud.local;192.168.1.4;443;\"path=/remote.php/webdav\"
=;wlp3s0;IPv4;Printer;_ipp._tcp;local;printer.local;192.168.1.5;631;
";
        let servers = parse_avahi(output);
        let summary: Vec<(&str, &str, &str)> = servers
            .iter()
            .map(|server| (server.name.as_str(), server.uri.as_str(), server.protocol))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("AG-LOCAL-CLOUD", "smb://ag-local-cloud.local/", "SMB"),
                ("Backup Box", "sftp://backup.local:2222/", "SFTP"),
                ("Cloud", "davs://cloud.local/remote.php/webdav", "WebDAV"),
                ("fritz-box", "ftp://fritz.box/", "FTP"),
            ]
        );
    }

    #[test]
    fn mount_messages_split_into_point_and_detail() {
        let (heading, body) = split_message("Identity Verification Failed\nThe host key changed.");
        assert_eq!(heading, "Identity Verification Failed");
        assert_eq!(body.as_deref(), Some("The host key changed."));
        let (heading, body) = split_message("Enter password for “me” on “host”:");
        assert_eq!(heading, "Enter password for “me” on “host”");
        assert!(body.is_none());
    }

    #[test]
    fn auth_failures_are_recognised() {
        assert!(looks_like_auth_failure(
            "Failed to mount Windows share: Permission denied"
        ));
        assert!(looks_like_auth_failure("Password dialog cancelled"));
        assert!(!looks_like_auth_failure("Connection refused"));
    }

    #[test]
    fn unescapes_avahi_names() {
        assert_eq!(
            unescape_avahi("Time\\032Capsule\\.local"),
            "Time Capsule.local"
        );
        assert_eq!(unescape_avahi("plain"), "plain");
    }
}

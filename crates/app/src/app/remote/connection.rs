//! Connection addresses are safe to persist; credentials are only passed to GVfs.
use super::*;

pub(super) struct Protocol {
    pub label: &'static str,
    pub scheme: &'static str,
    pub port: u16,
}

pub(super) const PROTOCOLS: &[Protocol] = &[
    Protocol {
        label: "SFTP · SSH",
        scheme: "sftp",
        port: 22,
    },
    Protocol {
        label: "FTP",
        scheme: "ftp",
        port: 21,
    },
    Protocol {
        label: "FTP · TLS",
        scheme: "ftps",
        port: 21,
    },
    Protocol {
        label: "SMB · Windows share",
        scheme: "smb",
        port: 445,
    },
    Protocol {
        label: "WebDAV · HTTPS",
        scheme: "davs",
        port: 443,
    },
    Protocol {
        label: "WebDAV · HTTP",
        scheme: "dav",
        port: 80,
    },
    Protocol {
        label: "AFP · Apple share",
        scheme: "afp",
        port: 548,
    },
    Protocol {
        label: "NFS",
        scheme: "nfs",
        port: 2049,
    },
    Protocol {
        label: "S3",
        scheme: "s3",
        port: 443,
    },
];

// Deliberately no derived Debug: Relm4 logs messages containing this value.
pub(crate) struct RemoteConnection {
    pub uri: String,
    pub(super) credentials: Option<Credentials>,
}

impl std::fmt::Debug for RemoteConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteConnection")
            .field("uri", &self.uri)
            .finish_non_exhaustive()
    }
}

pub(super) struct Credentials {
    pub username: String,
    pub password: Option<String>,
    pub domain: String,
    pub anonymous: bool,
    pub save: gio::PasswordSave,
}

impl Credentials {
    /// The backend can request credentials again after a rejected password. The
    /// caller consumes this value once, so retries always offer an editable prompt.
    pub fn reply_if_complete(
        &self,
        operation: &gio::MountOperation,
        flags: gio::AskPasswordFlags,
    ) -> bool {
        if self.anonymous {
            if !flags.contains(gio::AskPasswordFlags::ANONYMOUS_SUPPORTED) {
                return false;
            }
        } else if (flags.contains(gio::AskPasswordFlags::NEED_USERNAME) && self.username.is_empty())
            || (flags.contains(gio::AskPasswordFlags::NEED_PASSWORD) && self.password.is_none())
        {
            return false;
        }
        operation.set_anonymous(self.anonymous);
        if !self.anonymous {
            operation.set_username(Some(&self.username));
            operation.set_domain(Some(&self.domain));
            if flags.contains(gio::AskPasswordFlags::NEED_PASSWORD) {
                operation.set_password(self.password.as_deref());
            }
        }
        operation.set_password_save(if flags.contains(gio::AskPasswordFlags::SAVING_SUPPORTED) {
            self.save
        } else {
            gio::PasswordSave::Never
        });
        operation.reply(gio::MountOperationResult::Handled);
        true
    }
}

pub(super) struct ConnectionFields {
    pub protocol: usize,
    pub host: String,
    pub port: String,
    pub folder: String,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub anonymous: bool,
    pub save: gio::PasswordSave,
}

impl ConnectionFields {
    pub fn parse(address: &str) -> Result<Self, &'static str> {
        // Never propagate GLib's parse error: it may quote a password-bearing URL.
        let uri = glib::Uri::parse(address.trim(), glib::UriFlags::HAS_PASSWORD)
            .map_err(|_| "Enter a valid server address, such as ftp://server/folder.")?;
        let protocol = PROTOCOLS
            .iter()
            .position(|protocol| protocol.scheme == uri.scheme())
            .ok_or("Choose a supported connection type.")?;
        if uri.query().is_some() || uri.fragment().is_some() {
            return Err(
                "Server addresses cannot contain a query or fragment. Use the Folder field for names containing ? or #.",
            );
        }
        let mut username = uri.user().unwrap_or_default().to_string();
        let mut domain = String::new();
        if PROTOCOLS[protocol].scheme == "smb"
            && let Some((workgroup, user)) = username.split_once(';')
        {
            domain = workgroup.to_owned();
            username = user.to_owned();
        }
        let fields = Self {
            protocol,
            host: uri.host().unwrap_or_default().to_string(),
            port: if uri.port() == -1 {
                String::new()
            } else {
                uri.port().to_string()
            },
            folder: uri.path().to_string(),
            anonymous: matches!(PROTOCOLS[protocol].scheme, "ftp" | "ftps")
                && username == "anonymous",
            username,
            password: uri.password().unwrap_or_default().to_string(),
            domain,
            save: gio::PasswordSave::Never,
        };
        fields.connection()?;
        Ok(fields)
    }

    pub fn connection(&self) -> Result<RemoteConnection, &'static str> {
        let protocol = PROTOCOLS
            .get(self.protocol)
            .ok_or("Choose a connection type.")?;
        let host = self.host.trim();
        let host = host
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(host);
        if host.is_empty() {
            return Err("Enter a server name or IP address.");
        }
        if host.contains(['/', '@', '?', '#', '[', ']', '\\', '%'])
            || host.chars().any(char::is_whitespace)
            || host.chars().any(char::is_control)
            || (host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err())
        {
            return Err("Use a hostname or IP address; enter the port and folder separately.");
        }
        let port = if self.port.trim().is_empty() {
            -1
        } else {
            let value = self
                .port
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or("Port must be a number from 1 to 65535.")?;
            if value == protocol.port {
                -1
            } else {
                i32::from(value)
            }
        };
        let folder = if self.folder.is_empty() || self.folder == "/" {
            String::new()
        } else {
            format!("/{}", self.folder.trim_start_matches('/'))
        };
        let named = !self.anonymous && protocol.scheme != "nfs";
        let username = if named { self.username.trim() } else { "" };
        let user = if named && protocol.scheme == "smb" && !self.domain.trim().is_empty() {
            format!("{};{username}", self.domain.trim())
        } else if self.anonymous && matches!(protocol.scheme, "ftp" | "ftps") {
            "anonymous".to_owned()
        } else {
            username.to_owned()
        };
        let uri = glib::Uri::build_with_user(
            glib::UriFlags::NONE,
            protocol.scheme,
            (!user.is_empty()).then_some(user.as_str()),
            None,
            None,
            Some(host),
            port,
            &folder,
            None,
            None,
        )
        .to_str()
        .to_string();
        Ok(RemoteConnection {
            uri,
            credentials: (protocol.scheme != "nfs").then(|| Credentials {
                username: username.to_owned(),
                password: (named && !self.password.is_empty()).then(|| self.password.clone()),
                domain: if named {
                    self.domain.trim().to_owned()
                } else {
                    String::new()
                },
                anonymous: self.anonymous,
                save: self.save,
            }),
        })
    }
}

impl RemoteConnection {
    pub(in crate::app) fn parse(uri: &str) -> Result<Self, &'static str> {
        ConnectionFields::parse(uri)?.connection()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_decoded_but_never_saved_or_logged() {
        let fields = ConnectionFields::parse(
            "ftp://alice%40work:p%40ss%3A%2F%23%25@server:2121/My%20files/%C3%BC",
        )
        .unwrap();
        assert_eq!(fields.username, "alice@work");
        assert_eq!(fields.password, "p@ss:/#%");
        assert_eq!(fields.folder, "/My files/ü");
        let connection = fields.connection().unwrap();
        assert_eq!(
            connection.uri,
            "ftp://alice%40work@server:2121/My%20files/ü"
        );
        assert!(!format!("{connection:?}").contains("p@ss"));
        assert!(
            ConnectionFields::parse(&connection.uri)
                .unwrap()
                .password
                .is_empty()
        );
    }

    #[test]
    fn fields_escape_names_and_preserve_ipv6_and_smb_domain() {
        let mut fields =
            ConnectionFields::parse("smb://WORK;alice@[2001:db8::1]:1445/share").unwrap();
        assert_eq!(fields.domain, "WORK");
        assert_eq!(fields.username, "alice");
        fields.folder = "/share/100% ready/#draft?".to_owned();
        let connection = fields.connection().unwrap();
        let decoded = ConnectionFields::parse(&connection.uri).unwrap();
        assert_eq!(decoded.host, "2001:db8::1");
        assert_eq!(decoded.port, "1445");
        assert_eq!(decoded.folder, fields.folder);
        assert_eq!(decoded.domain, "WORK");
        assert_eq!(decoded.username, "alice");
    }

    #[test]
    fn validation_rejects_bad_servers_ports_and_ambiguous_urls() {
        for uri in [
            "ftp://",
            "https://host",
            "ftp://host:65536",
            "ftp://host:0",
            "ftp://host?token=secret",
            "ftp://host/#folder",
        ] {
            assert!(ConnectionFields::parse(uri).is_err(), "{uri}");
        }
        let mut fields = ConnectionFields::parse("ftp://server").unwrap();
        for host in [
            "server:21",
            "user@host",
            "host/path",
            "space host",
            "host\\path",
        ] {
            fields.host = host.to_owned();
            assert!(fields.connection().is_err(), "{host}");
        }
    }

    #[test]
    fn defaults_and_anonymous_login_have_stable_saved_addresses() {
        for protocol in PROTOCOLS {
            let connection = RemoteConnection::parse(&format!(
                "{}://server:{}/",
                protocol.scheme, protocol.port
            ))
            .unwrap();
            assert_eq!(connection.uri, format!("{}://server", protocol.scheme));
        }
        let fields = ConnectionFields::parse("ftp://anonymous@server/").unwrap();
        assert!(fields.anonymous);
        assert!(fields.connection().unwrap().credentials.unwrap().anonymous);
    }

    #[test]
    fn credentials_respect_backend_requirements_and_password_storage() {
        let operation = gio::MountOperation::new();
        let replied = Rc::new(Cell::new(false));
        let seen = replied.clone();
        operation
            .connect_reply(move |_, result| seen.set(result == gio::MountOperationResult::Handled));
        let mut login = Credentials {
            username: "alice".into(),
            password: None,
            domain: "WORK".into(),
            anonymous: false,
            save: gio::PasswordSave::Permanently,
        };
        let flags = gio::AskPasswordFlags::NEED_USERNAME | gio::AskPasswordFlags::NEED_PASSWORD;
        assert!(!login.reply_if_complete(&operation, flags));
        assert!(!replied.get());
        login.password = Some("test-password".into());
        assert!(login.reply_if_complete(&operation, flags));
        assert!(replied.get());
        assert_eq!(operation.password().as_deref(), Some("test-password"));
        assert_eq!(operation.password_save(), gio::PasswordSave::Never);
        assert!(
            login.reply_if_complete(&operation, flags | gio::AskPasswordFlags::SAVING_SUPPORTED)
        );
        assert_eq!(operation.password_save(), gio::PasswordSave::Permanently);
        login.anonymous = true;
        assert!(!login.reply_if_complete(&operation, flags));
        assert!(login.reply_if_complete(
            &operation,
            flags | gio::AskPasswordFlags::ANONYMOUS_SUPPORTED
        ));
        assert!(operation.is_anonymous());
    }
}

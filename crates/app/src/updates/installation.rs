use std::path::Path;
use std::process::{Command, Stdio};

use super::{Format, numeric_version};

#[derive(Clone, Debug)]
pub(crate) struct Installation {
    pub preferred: Option<Format>,
    pub description: String,
}

impl Installation {
    pub fn detect() -> Self {
        if std::env::var_os("APPIMAGE").is_some_and(|path| Path::new(&path).is_file()) {
            return Self { preferred: Some(Format::AppImage), description:
                "AppImage installation. Download the new AppImage and replace it after closing Commander.".into() };
        }
        if std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some() {
            return Self {
                preferred: None,
                description:
                    "Managed by your software store. Use the store to update this installation."
                        .into(),
            };
        }
        if let Ok(executable) = std::env::current_exe() {
            for (binary, args, format) in [
                ("/usr/bin/dpkg-query", vec!["-S", "--"], Format::Deb),
                (
                    "/usr/bin/rpm",
                    vec!["-qf", "--qf", "%{NAME}", "--"],
                    Format::Rpm,
                ),
                ("/usr/bin/pacman", vec!["-Qqo", "--"], Format::Arch),
            ] {
                if !Path::new(binary).is_file() {
                    continue;
                }
                if Command::new(binary)
                    .args(args)
                    .arg(&executable)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
                {
                    return Self { preferred: Some(format), description:
                        "Managed by your system package manager. Prefer its updates; a downloaded package can be installed manually.".into() };
                }
            }
        }
        Self { preferred: None, description:
            "Source build or unrecognized installation. Choose a package for a manual installation, or rebuild from the release tag.".into() }
    }
}

pub(super) fn host_glibc() -> Option<Vec<u32>> {
    let output = Command::new("/usr/bin/getconf")
        .arg("GNU_LIBC_VERSION")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    numeric_version(text.trim().strip_prefix("glibc ")?)
}

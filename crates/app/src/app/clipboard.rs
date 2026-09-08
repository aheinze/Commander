//! Desktop file clipboard: GNOME and KDE cut markers plus standard URI/file lists.
use super::*;
const GNOME: &str = "x-special/gnome-copied-files";
const URIS: &str = "text/uri-list";
const KDE_CUT: &str = "application/x-kde-cutselection";
const MAX_CLIPBOARD_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn provider(paths: &[VPath], cut: bool) -> Option<gdk::ContentProvider> {
    let files = pane_view::file_list_provider(paths)?;
    let uris = paths
        .iter()
        .map(|path| gio::File::for_path(path.as_path()).uri().to_string())
        .collect::<Vec<_>>();
    let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
    Some(gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_bytes(GNOME, &glib::Bytes::from_owned(gnome.into_bytes())),
        gdk::ContentProvider::for_bytes(
            URIS,
            &glib::Bytes::from_owned(format!("{}\r\n", uris.join("\r\n")).into_bytes()),
        ),
        gdk::ContentProvider::for_bytes(
            KDE_CUT,
            &glib::Bytes::from_static(if cut { b"1" } else { b"0" }),
        ),
        files,
    ]))
}

fn parse(bytes: &[u8], gnome: bool) -> Result<(Vec<VPath>, bool), String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "The file clipboard contains invalid URI data")?;
    let mut lines = text.lines();
    let cut = if gnome {
        match lines.next() {
            Some("cut") => true,
            Some("copy") => false,
            _ => return Err("The file clipboard has an invalid copy/cut marker".to_owned()),
        }
    } else {
        false
    };
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for uri in lines.filter(|line| !line.is_empty() && !line.starts_with('#')) {
        if !uri.starts_with("file:") {
            return Err("Connect the remote folder before pasting its files".to_owned());
        }
        let path = gio::File::for_uri(uri)
            .path()
            .filter(|path| path.is_absolute())
            .ok_or("The clipboard contains a file address that is not available locally")?;
        let path = VPath::from(path);
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err("The clipboard does not contain files".to_owned());
    }
    Ok((paths, cut))
}

async fn read_mime(clipboard: &gdk::Clipboard, mime: &str) -> Result<Vec<u8>, String> {
    let (stream, _) = clipboard
        .read_future(&[mime], glib::Priority::DEFAULT)
        .await
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await
            .map_err(|error| error.to_string())?;
        if chunk.is_empty() {
            break;
        }
        if bytes.len() + chunk.len() > MAX_CLIPBOARD_BYTES {
            return Err("The file clipboard is too large (limit 64 MiB)".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_files(clipboard: &gdk::Clipboard) -> Result<(Vec<VPath>, bool), String> {
    let formats = clipboard.formats();
    if formats.contain_mime_type(GNOME) {
        let bytes = read_mime(clipboard, GNOME).await?;
        return gio::spawn_blocking(move || parse(&bytes, true))
            .await
            .map_err(|_| "Could not decode the file clipboard".to_owned())?;
    }
    let mut result = if formats.contain_mime_type(URIS) {
        let bytes = read_mime(clipboard, URIS).await?;
        gio::spawn_blocking(move || parse(&bytes, false))
            .await
            .map_err(|_| "Could not decode the file clipboard".to_owned())??
    } else {
        let value = clipboard
            .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
            .await
            .map_err(|_| "The clipboard does not contain files")?;
        (
            pane_view::paths_from_file_list(&value)
                .ok_or("Some clipboard files are unavailable locally")?,
            false,
        )
    };
    if formats.contain_mime_type(KDE_CUT) {
        result.1 = read_mime(clipboard, KDE_CUT).await? == b"1";
    }
    Ok(result)
}

impl AppModel {
    pub(super) fn paste_file_clipboard(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.active_pane;
        let destination = self.pane(pane).current_directory().clone();
        self.paste_file_clipboard_into(pane, destination, sender);
    }

    pub(super) fn paste_file_clipboard_into(
        &mut self,
        pane: PaneId,
        destination: VPath,
        sender: &ComponentSender<Self>,
    ) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let owner = self
            .clipboard_provider
            .as_ref()
            .filter(|provider| clipboard.content().as_ref() == Some(*provider))
            .map(|_| self.clipboard_generation);
        let input = sender.input_sender().clone();
        glib::spawn_future_local(async move {
            let changed = Rc::new(Cell::new(false));
            let changed_signal = changed.clone();
            let handler = clipboard.connect_changed(move |_| changed_signal.set(true));
            let mut result =
                glib::future_with_timeout(Duration::from_secs(10), read_files(&clipboard))
                    .await
                    .map_err(|_| "Reading the file clipboard timed out".to_owned())
                    .and_then(|result| result);
            clipboard.disconnect(handler);
            if changed.get() {
                result = Err("The clipboard changed while being read; paste again".to_owned());
            }
            let _ = input.send(AppMsg::ClipboardFiles {
                pane,
                destination,
                result,
                owner,
            });
        });
    }
}

pub(super) fn remaining_cut_sources(sources: &[VPath], transfers: &[TransferRecord]) -> Vec<VPath> {
    let removed = transfers
        .iter()
        .filter(|record| record.source_removed)
        .map(|record| &record.source)
        .collect::<BTreeSet<_>>();
    sources
        .iter()
        .filter(|source| !removed.contains(source))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_cut_items_stay_on_the_clipboard() {
        let fixture = tempfile::tempdir().unwrap();
        let one = VPath::from(fixture.path().join("one"));
        let two = VPath::from(fixture.path().join("two"));
        std::fs::write(one.as_path(), b"one").unwrap();
        let record = TransferRecord {
            source: one.clone(),
            destination: VPath::from("/destination/one"),
            metadata: LocalFs.stat(&one, false).unwrap(),
            created: true,
            source_removed: true,
        };
        assert_eq!(
            remaining_cut_sources(&[one.clone(), two.clone()], &[record]),
            vec![two]
        );
        assert_eq!(
            remaining_cut_sources(std::slice::from_ref(&one), &[]),
            vec![one]
        );
    }
    #[test]
    fn uri_clipboard_handles_comments_escaping_and_cut() {
        let (paths, cut) = parse(
            b"cut\nfile:///tmp/a%20b\nfile:///tmp/line%0Abreak\nfile:///tmp/a%20b\n",
            true,
        )
        .unwrap();
        assert!(cut);
        assert_eq!(
            paths,
            vec![VPath::from("/tmp/a b"), VPath::from("/tmp/line\nbreak")]
        );
        assert_eq!(
            parse(b"# comment\r\nfile:///tmp/a\r\n", false).unwrap(),
            (vec![VPath::from("/tmp/a")], false)
        );
    }
    #[test]
    fn malformed_or_remote_clipboards_never_partially_paste() {
        assert!(parse(b"move\nfile:///tmp/a", true).is_err());
        assert!(parse(b"file:///tmp/a\nsftp://server/b", false).is_err());
        assert!(parse(b"/tmp/a", false).is_err());
        assert!(parse(b"", false).is_err());
    }
}

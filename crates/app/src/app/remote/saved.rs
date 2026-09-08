use super::*;

/// Compare protocol, login, port and path boundaries, never the display name.
pub(in crate::app) fn relative_mount_path(
    mount_uri: &str,
    saved_uri: &str,
) -> Option<std::path::PathBuf> {
    let file = |uri| {
        let mut fields = super::connection::ConnectionFields::parse(uri).ok()?;
        fields.host.make_ascii_lowercase();
        Some(gio::File::for_uri(&fields.connection().ok()?.uri))
    };
    let root = file(mount_uri)?;
    let saved = file(saved_uri)?;
    if root.equal(&saved) {
        Some(std::path::PathBuf::new())
    } else {
        root.relative_path(&saved)
    }
}

/// Replace the captured saved address in place. Only normalized, password-free URIs
/// belong in session storage; the system keyring remains owned by GVfs.
pub(in crate::app) fn save_location(
    locations: &mut Vec<String>,
    names: &mut BTreeMap<String, String>,
    address: &str,
    replacing: Option<&str>,
    name: Option<&str>,
) -> Result<(), &'static str> {
    let connection = RemoteConnection::parse(address)?;
    if let Some(old) = replacing {
        let index = locations
            .iter()
            .position(|uri| uri == old)
            .ok_or("This saved connection was removed or changed. Open its editor again.")?;
        locations[index] = connection.uri.clone();
        let mut position = 0;
        locations.retain(|uri| {
            let keep = position == index || uri != &connection.uri;
            position += 1;
            keep
        });
    } else if !locations.contains(&connection.uri) {
        locations.push(connection.uri.clone());
    }
    // Keep names separate from connection URIs and move them with edited addresses.
    let previous_name = replacing.and_then(|old| names.remove(old));
    if let Some(name) = name.map(str::trim).map(str::to_owned).or(previous_name) {
        if name.is_empty() {
            names.remove(&connection.uri);
        } else {
            names.insert(connection.uri, name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

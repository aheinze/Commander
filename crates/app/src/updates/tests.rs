use super::*;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;

fn asset() -> Asset {
    Asset {
        name: "commander-1.0.0-1-x86_64.AppImage".into(),
        architecture: "x86_64".into(),
        format: Format::AppImage,
        size: 7,
        sha256: format!("{:x}", Sha256::digest(b"package")),
        glibc_minimum: "2.35".into(),
    }
}

fn manifest() -> Manifest {
    Manifest {
        schema: 1,
        repository: REPOSITORY.into(),
        version: Version::parse("1.0.0").unwrap(),
        tag: "v1.0.0".into(),
        commit: "a".repeat(40),
        assets: vec![asset()],
    }
}

fn signed(value: &Manifest) -> (Vec<u8>, Vec<u8>, String) {
    let signing = SigningKey::from_bytes(&[7; 32]); // Test-only key, never a release trust root.
    let bytes = serde_json::to_vec(value).unwrap();
    let signature = signing.sign(&bytes).to_bytes().to_vec();
    let key = signing
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (bytes, signature, key)
}

#[test]
fn signed_manifest_binds_release_and_rejects_tampering_and_unsafe_assets() {
    let original = manifest();
    let (bytes, signature, key) = signed(&original);
    assert!(verified_manifest(&bytes, &signature, &key, &original.version).is_ok());
    let mut modified = bytes.clone();
    modified[0] ^= 1;
    assert!(verified_manifest(&modified, &signature, &key, &original.version).is_err());
    assert!(verified_manifest(&bytes, &signature[..63], &key, &original.version).is_err());
    assert!(verified_manifest(&bytes, &signature, &"00".repeat(32), &original.version).is_err());
    assert!(
        verified_manifest(&bytes, &signature, &key, &Version::parse("2.0.0").unwrap()).is_err()
    );
    for change in 0..8 {
        let mut value = manifest();
        match change {
            0 => value.repository = "other/repo".into(),
            1 => value.schema = 2,
            2 => value.assets[0].name = "../elsewhere.AppImage".into(),
            3 => value.assets.push(asset()),
            4 => value.assets[0].size = MAX_DOWNLOAD + 1,
            5 => value.assets[0].sha256 = "not a digest".into(),
            6 => value.assets[0].glibc_minimum = "2.x".into(),
            _ => value.assets[0].format = Format::Deb,
        }
        let (bytes, signature, key) = signed(&value);
        assert!(
            verified_manifest(&bytes, &signature, &key, &original.version).is_err(),
            "accepted mutation {change}"
        );
    }
}

struct Fake {
    replies: RefCell<VecDeque<github::Response>>,
    requests: RefCell<Vec<(String, Option<String>)>>,
}

impl github::Transport for Fake {
    fn get(&self, url: &str, etag: Option<&str>, maximum: u64) -> Result<github::Response> {
        self.requests
            .borrow_mut()
            .push((url.into(), etag.map(str::to_owned)));
        let reply = self
            .replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| Failure::new("Offline"))?;
        assert!(reply.body.len() as u64 <= maximum);
        Ok(reply)
    }
}

fn response(status: u16, body: Vec<u8>) -> github::Response {
    github::Response {
        status,
        body,
        etag: Some("\"release-1\"".into()),
        retry_at: None,
    }
}

fn release(version: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
        "body": "# Release notes\nA useful improvement.", "assets": [
            {"name": "update.json", "size": 500, "state": "uploaded"},
            {"name": "update.json.sig", "size": 64, "state": "uploaded"},
            {"name": asset().name, "size": 7, "state": "uploaded"}
        ]
    }))
    .unwrap()
}

fn fake(replies: Vec<github::Response>) -> Fake {
    Fake {
        replies: RefCell::new(replies.into()),
        requests: RefCell::default(),
    }
}

fn installation() -> Installation {
    Installation {
        preferred: None,
        description: "Test installation".into(),
    }
}

fn check(client: &Fake, cache: &mut Cache, key: Option<&str>, current: &str) -> Result<Outcome> {
    github::check_with(
        client,
        cache,
        &CancelToken::new(),
        key,
        current,
        "x86_64",
        Some(vec![2, 35]),
        installation(),
    )
}

#[test]
fn checks_use_semantic_precedence_and_never_offer_downgrades_or_prereleases() {
    for (remote, current, available) in [
        ("0.10.0", "0.9.0", true),
        ("1.0.0", "1.0.0-rc.1", true),
        ("0.9.0", "0.10.0", false),
        ("1.0.0", "1.0.0+local", false),
    ] {
        let result = check(
            &fake(vec![response(200, release(remote))]),
            &mut Cache::default(),
            None,
            current,
        )
        .unwrap();
        assert_eq!(matches!(result, Outcome::Available(_)), available);
    }
    assert!(
        check(
            &fake(vec![response(200, release("1.1.0-rc.1"))]),
            &mut Cache::default(),
            None,
            "1.0.0"
        )
        .is_err()
    );
}

#[test]
fn authenticated_assets_are_bound_to_github_and_conditional_cache_is_reused() {
    let (bytes, signature, key) = signed(&manifest());
    let client = fake(vec![
        response(200, release("1.0.0")),
        response(200, bytes.clone()),
        response(200, signature.clone()),
        response(304, vec![]),
        response(200, bytes),
        response(200, signature),
    ]);
    let mut cache = Cache::default();
    let Outcome::Available(update) = check(&client, &mut cache, Some(&key), "0.4.0").unwrap()
    else {
        panic!()
    };
    assert_eq!(update.assets.len(), 1);
    assert!(update.notice.is_empty());
    assert_eq!(
        update.download_url(&update.assets[0]),
        format!("{RELEASES_URL}/download/v1.0.0/{}", asset().name)
    );
    assert!(cache.checked_at.is_some());
    assert!(check(&client, &mut cache, Some(&key), "0.4.0").is_err());
    assert_eq!(
        client.requests.borrow().len(),
        3,
        "manual cooldown must not poll GitHub"
    );
    cache.retry_at = None;
    assert!(check(&client, &mut cache, Some(&key), "0.4.0").is_ok());
    assert_eq!(
        client.requests.borrow()[3].1.as_deref(),
        Some("\"release-1\"")
    );
}

#[test]
fn errors_no_releases_unsigned_and_incompatible_builds_have_distinct_results() {
    let mut cache = Cache::default();
    let mut limited = response(429, vec![]);
    limited.retry_at = Some(now() + 3600);
    let client = fake(vec![limited]);
    assert!(check(&client, &mut cache, None, "0.4.0").is_err());
    assert!(cache.checked_at.is_none());
    assert!(cache.retry_at.unwrap() > now() + 3000);
    assert!(matches!(
        check(
            &fake(vec![response(404, vec![])]),
            &mut Cache::default(),
            None,
            "0.4.0"
        ),
        Ok(Outcome::NoRelease)
    ));
    assert!(check(&fake(vec![]), &mut Cache::default(), None, "0.4.0").is_err());
    assert!(
        check(
            &fake(vec![response(200, b"not json".to_vec())]),
            &mut Cache::default(),
            None,
            "0.4.0"
        )
        .is_err()
    );
    let Outcome::Available(unsigned) = check(
        &fake(vec![response(200, release("1.0.0"))]),
        &mut Cache::default(),
        None,
        "0.4.0",
    )
    .unwrap() else {
        panic!()
    };
    assert!(unsigned.assets.is_empty());
    assert!(unsigned.notice.contains("verification key"));
    let (bytes, signature, key) = signed(&manifest());
    for (architecture, glibc) in [
        ("aarch64", Some(vec![2, 35])),
        ("x86_64", Some(vec![2, 34])),
        ("x86_64", None),
    ] {
        let client = fake(vec![
            response(200, release("1.0.0")),
            response(200, bytes.clone()),
            response(200, signature.clone()),
        ]);
        let Outcome::Available(update) = github::check_with(
            &client,
            &mut Cache::default(),
            &CancelToken::new(),
            Some(&key),
            "0.4.0",
            architecture,
            glibc,
            installation(),
        )
        .unwrap() else {
            panic!()
        };
        assert!(update.assets.is_empty());
        assert!(update.notice.contains("No verified package"));
    }
    let client = fake(vec![
        response(200, release("1.0.0")),
        response(200, bytes),
        response(200, vec![0; 64]),
    ]);
    assert!(check(&client, &mut Cache::default(), Some(&key), "0.4.0").is_err());
}

#[test]
fn downloads_publish_only_complete_verified_files_and_never_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("update.AppImage");
    for body in [
        b"bad data".as_slice(),
        b"pack",
        b"packagE",
        b"package too long",
    ] {
        assert!(
            download::receive(
                Cursor::new(body),
                &asset(),
                &path,
                &CancelToken::new(),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    let cancel = CancelToken::new();
    assert!(
        download::receive(Cursor::new(b"package"), &asset(), &path, &cancel, |_| {
            cancel.cancel()
        })
        .is_err()
    );
    assert!(!path.exists());
    assert!(
        download::receive(
            Cursor::new(b"package"),
            &asset(),
            &path,
            &CancelToken::new(),
            |_| {}
        )
        .is_ok()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"package");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert!(
        download::receive(
            Cursor::new(b"package"),
            &asset(),
            &path,
            &CancelToken::new(),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"package");
    std::fs::remove_file(&path).unwrap();
    assert!(
        download::receive(
            Cursor::new(b"package"),
            &asset(),
            &path,
            &CancelToken::new(),
            |_| {
                std::fs::write(&path, b"user file").unwrap();
            }
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"user file");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn cache_roundtrips_and_ignores_corrupt_or_oversized_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.json");
    let cache = Cache {
        skipped: Some("1.0.0".into()),
        checked_at: Some(100),
        etag: Some("etag".into()),
        ..Cache::default()
    };
    cache.save(&path).unwrap();
    assert_eq!(Cache::load(&path).skipped, cache.skipped);
    std::fs::write(&path, b"bad json").unwrap();
    assert!(Cache::load(&path).checked_at.is_none());
    assert!(read_bounded(Cursor::new(vec![0; 11]), 10).is_err());
}

#[test]
fn github_inventory_mismatch_and_cancelled_checks_never_authorize_downloads() {
    let (bytes, signature, key) = signed(&manifest());
    let mut remote: serde_json::Value = serde_json::from_slice(&release("1.0.0")).unwrap();
    remote["assets"][2]["size"] = 8.into();
    let client = fake(vec![
        response(200, serde_json::to_vec(&remote).unwrap()),
        response(200, bytes),
        response(200, signature),
    ]);
    assert!(check(&client, &mut Cache::default(), Some(&key), "0.4.0").is_err());
    let cancel = CancelToken::new();
    cancel.cancel();
    let client = fake(vec![]);
    assert!(
        github::check_with(
            &client,
            &mut Cache::default(),
            &cancel,
            Some(&key),
            "0.4.0",
            "x86_64",
            Some(vec![2, 35]),
            installation()
        )
        .is_err()
    );
    assert!(client.requests.borrow().is_empty());
}

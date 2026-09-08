use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=COMMANDER_UPDATE_PUBLIC_KEY");
    if let Ok(key) = env::var("COMMANDER_UPDATE_PUBLIC_KEY") {
        assert!(
            key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "COMMANDER_UPDATE_PUBLIC_KEY must be a 32-byte Ed25519 public key in hexadecimal"
        );
    }
    println!("cargo:rerun-if-changed=assets/icons.gresource.xml");
    println!("cargo:rerun-if-changed=assets/icons");
    println!("cargo:rerun-if-changed=assets/branding/commander.svg");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("icons.gresource");
    let status = Command::new("glib-compile-resources")
        .arg("assets/icons.gresource.xml")
        .arg("--sourcedir=assets")
        .arg("--target")
        .arg(output)
        .status()
        .expect("install the GLib development tools (glib-compile-resources) to build Commander");
    assert!(
        status.success(),
        "could not compile the bundled icon resources"
    );
}

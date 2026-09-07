use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/icons.gresource.xml");
    println!("cargo:rerun-if-changed=assets/icons");
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

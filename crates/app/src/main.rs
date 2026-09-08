#![forbid(unsafe_code)]

mod app;
mod archive;
mod cli;
mod commands;
mod features;
mod history_store;
mod icons;
mod list_model;
mod omarchy;
mod pdf;
mod session;
mod terminal;
mod updates;

use std::error::Error;
use std::time::Instant;

use relm4::RelmApp;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

use app::{AppInit, AppModel};
use cli::ParseOutcome;
use session::SessionWorker;

pub(crate) const APP_NAME: &str = "Commander";

fn main() {
    if let Err(error) = run() {
        eprintln!("{APP_NAME}: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    relm4::gtk::glib::set_application_name(APP_NAME);
    let started = Instant::now();
    let options = match cli::parse()? {
        ParseOutcome::Help => {
            println!("{}", cli::help());
            return Ok(());
        }
        ParseOutcome::Version => {
            println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        ParseOutcome::Run(options) => options,
    };
    setup_tracing(options.profile_startup)?;
    let (session_worker, startup) = SessionWorker::start()?;
    tracing::info!(
        platform = dualpane_platform::platform_family(),
        "starting Commander"
    );

    let gtk_args = vec![
        std::env::args()
            .next()
            .unwrap_or_else(|| "commander".to_owned()),
    ];
    RelmApp::new("org.example.Dualpane")
        .with_args(gtk_args)
        .run::<AppModel>(AppInit {
            options,
            session_worker,
            session: startup.session,
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started,
        });
    Ok(())
}

#[cfg(not(feature = "tracy"))]
fn setup_tracing(profile_startup: bool) -> Result<(), Box<dyn Error>> {
    let default = if profile_startup {
        "commander=debug,dualpane=debug"
    } else {
        "commander=info,dualpane=info"
    };
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default)))
        .with(tracing_subscriber::fmt::layer())
        .try_init()?;
    Ok(())
}

#[cfg(feature = "tracy")]
fn setup_tracing(profile_startup: bool) -> Result<(), Box<dyn Error>> {
    let default = if profile_startup {
        "commander=debug,dualpane=debug"
    } else {
        "commander=info,dualpane=info"
    };
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default)))
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_tracy::TracyLayer::default())
        .try_init()?;
    Ok(())
}

use std::ffi::OsString;
use std::path::PathBuf;

use dualpane_core::VPath;
use thiserror::Error;

/// Startup options intentionally parsed without a heavy command-line dependency.
#[derive(Clone, Debug, Default)]
pub struct AppOptions {
    pub left: Option<VPath>,
    pub right: Option<VPath>,
    pub profile_startup: bool,
    pub benchmark_mode: bool,
    pub benchmark_filter: Option<String>,
    pub quit_after_first_paint: bool,
}

/// Result of parsing arguments.
#[derive(Clone, Debug)]
pub enum ParseOutcome {
    Run(AppOptions),
    Help,
    Version,
}

/// Command-line parsing errors.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("missing value after {0}")]
    MissingValue(&'static str),
    #[error("unknown option: {0}")]
    UnknownOption(String),
}

/// Parses process arguments.
///
/// # Errors
///
/// Returns missing-value and unknown-option failures.
pub fn parse() -> Result<ParseOutcome, CliError> {
    parse_from(std::env::args_os().skip(1))
}

fn parse_from(arguments: impl IntoIterator<Item = OsString>) -> Result<ParseOutcome, CliError> {
    let mut options = AppOptions::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--left") => {
                options.left = Some(VPath::from(PathBuf::from(
                    arguments.next().ok_or(CliError::MissingValue("--left"))?,
                )));
            }
            Some("--right") => {
                options.right = Some(VPath::from(PathBuf::from(
                    arguments.next().ok_or(CliError::MissingValue("--right"))?,
                )));
            }
            Some("--benchmark") => {
                let path = VPath::from(PathBuf::from(
                    arguments
                        .next()
                        .ok_or(CliError::MissingValue("--benchmark"))?,
                ));
                options.left = Some(path.clone());
                options.right = Some(path);
                options.profile_startup = true;
                options.benchmark_mode = true;
            }
            Some("--profile-startup") => options.profile_startup = true,
            Some("--benchmark-filter") => {
                options.benchmark_filter = Some(
                    arguments
                        .next()
                        .ok_or(CliError::MissingValue("--benchmark-filter"))?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Some("--quit-after-first-paint") => options.quit_after_first_paint = true,
            Some("--help" | "-h") => return Ok(ParseOutcome::Help),
            Some("--version" | "-V") => return Ok(ParseOutcome::Version),
            Some(value) => return Err(CliError::UnknownOption(value.to_owned())),
            None => {
                return Err(CliError::UnknownOption(
                    argument.to_string_lossy().into_owned(),
                ));
            }
        }
    }
    Ok(ParseOutcome::Run(options))
}

pub fn help() -> &'static str {
    "Commander native file manager\n\nUSAGE:\n    commander [OPTIONS]\n\nOPTIONS:\n    --left PATH                  Initial left-pane directory\n    --right PATH                 Initial right-pane directory\n    --profile-startup            Emit startup/listing timing spans\n    --benchmark PATH             Open PATH in both panes for profiling\n    --benchmark-filter QUERY     Measure type-ahead during the UI benchmark\n    --quit-after-first-paint     Exit after both panes paint their first listing\n    -h, --help                   Show this help\n    -V, --version                Show the Commander version"
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{ParseOutcome, parse_from};

    #[test]
    fn version_flag_returns_without_starting_the_app() {
        assert!(matches!(
            parse_from([OsString::from("--version")]).expect("parse"),
            ParseOutcome::Version
        ));
        assert!(matches!(
            parse_from([OsString::from("-V")]).expect("parse"),
            ParseOutcome::Version
        ));
    }

    #[test]
    fn benchmark_sets_both_panes_and_profiling() {
        let outcome =
            parse_from([OsString::from("--benchmark"), OsString::from("/tmp")]).expect("parse");
        let ParseOutcome::Run(options) = outcome else {
            panic!("unexpected help");
        };

        assert_eq!(options.left.as_ref().unwrap().to_string(), "/tmp");
        assert_eq!(options.right.as_ref().unwrap().to_string(), "/tmp");
        assert!(options.profile_startup);
        assert!(options.benchmark_mode);
    }

    #[test]
    fn benchmark_filter_keeps_the_query() {
        let outcome = parse_from([
            OsString::from("--benchmark-filter"),
            OsString::from("entry-9876"),
        ])
        .expect("parse");
        let ParseOutcome::Run(options) = outcome else {
            panic!("unexpected help");
        };
        assert_eq!(options.benchmark_filter.as_deref(), Some("entry-9876"));
    }
}

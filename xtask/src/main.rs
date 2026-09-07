#![forbid(unsafe_code)]

mod budgets;
mod fixtures;
mod m4;

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use budgets::check_m1;
use dualpane_core::{CancelToken, VPath};
use fixtures::{FixtureOptions, generate};
use m4::{M4BenchOptions, check_m4};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let Some(command) = arguments.next() else {
        print_help();
        return Ok(());
    };

    match command.as_str() {
        "check-m1-budgets" => run_check_m1_budgets(arguments.collect()),
        "check-m4-budgets" => run_check_m4_budgets(arguments.collect()),
        "gen-fixtures" => run_gen_fixtures(arguments.collect()),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        unknown => Err(format!("unknown command `{unknown}`; run `cargo xtask help`").into()),
    }
}

fn run_check_m4_budgets(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let mut options = M4BenchOptions::default();
    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--large" => options.large_file = PathBuf::from(next_value(&mut arguments, &flag)?),
            "--small" => options.small_tree = PathBuf::from(next_value(&mut arguments, &flag)?),
            "--work" => options.work_root = PathBuf::from(next_value(&mut arguments, &flag)?),
            "--help" | "-h" => {
                println!(
                    "Compare M4 copy paths with cp.\n\nUSAGE:\n    cargo run --release -p xtask -- \\\n+                     check-m4-budgets [--large PATH] [--small PATH] [--work PATH]"
                );
                return Ok(());
            }
            unknown => return Err(format!("unknown check-m4-budgets option `{unknown}`").into()),
        }
    }
    let report = check_m4(&options)?;
    println!(
        "M4 large: engine={:.3} ms scan={:.3} ms cp={:.3} ms ratio={:.3} method={:?} \
         bytes_read={}",
        report.large_engine.as_secs_f64() * 1_000.0,
        report.large_scan.as_secs_f64() * 1_000.0,
        report.large_cp.as_secs_f64() * 1_000.0,
        report.large_ratio,
        report.large_method,
        report.large_bytes_read,
    );
    println!(
        "M4 small: engine={:.3} s scan={:.3} s cp={:.3} s rate_ratio={:.3}",
        report.small_engine.as_secs_f64(),
        report.small_scan.as_secs_f64(),
        report.small_cp.as_secs_f64(),
        report.small_rate_ratio,
    );
    report.enforce()?;
    println!("M4 copy budgets passed");
    Ok(())
}

fn run_check_m1_budgets(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let mut root = PathBuf::from("target/fixtures/flat-100k/bucket-000000");
    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--root" => root = PathBuf::from(next_value(&mut arguments, &flag)?),
            "--help" | "-h" => {
                println!(
                    "Enforce M1 release budgets.\n\nUSAGE:\n    cargo xtask check-m1-budgets [--root PATH]"
                );
                return Ok(());
            }
            unknown => return Err(format!("unknown check-m1-budgets option `{unknown}`").into()),
        }
    }

    let report = check_m1(&VPath::from(root))?;
    println!(
        "M1 budgets: first_snapshot={:.3} ms, complete_listing_sort={:.3} ms, \
         incremental_filter={:.3} ms, complete_listing_sort_stat={:.3} ms",
        report.first_snapshot.as_secs_f64() * 1_000.0,
        report.listing_sort.as_secs_f64() * 1_000.0,
        report.incremental_filter.as_secs_f64() * 1_000.0,
        report.listing_sort_stat.as_secs_f64() * 1_000.0,
    );
    report.enforce()?;
    println!("M1 performance budgets passed");
    Ok(())
}

fn run_gen_fixtures(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print_gen_fixtures_help();
        return Ok(());
    }

    let options = parse_fixture_options(arguments)?;
    let report = generate(&options, &CancelToken::new())?;
    let seconds = report.elapsed.as_secs_f64();
    let rate = if seconds > 0.0 {
        report.files as f64 / seconds
    } else {
        0.0
    };

    println!(
        "generated {} files in {} directories at {} ({} bytes, {:.3} s, {:.0} files/s)",
        report.files,
        report.directories,
        report.root.display(),
        report.bytes_written,
        seconds,
        rate
    );

    Ok(())
}

fn parse_fixture_options(arguments: Vec<String>) -> Result<FixtureOptions, Box<dyn Error>> {
    let mut options = FixtureOptions::default();
    let mut arguments = arguments.into_iter();

    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--root" => options.root = PathBuf::from(next_value(&mut arguments, &flag)?),
            "--files" => options.files = parse_number(next_value(&mut arguments, &flag)?, &flag)?,
            "--files-per-directory" => {
                options.files_per_directory =
                    parse_number(next_value(&mut arguments, &flag)?, &flag)?;
            }
            "--payload-bytes" => {
                options.payload_bytes = parse_number(next_value(&mut arguments, &flag)?, &flag)?;
            }
            unknown => return Err(format!("unknown gen-fixtures option `{unknown}`").into()),
        }
    }

    Ok(options)
}

fn next_value(
    arguments: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn Error>> {
    arguments
        .next()
        .ok_or_else(|| format!("missing value after `{flag}`").into())
}

fn parse_number<T>(value: String, flag: &str) -> Result<T, Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: Error + 'static,
{
    value
        .parse()
        .map_err(|error| format!("invalid value for `{flag}`: {error}").into())
}

fn print_help() {
    println!(
        "Commander repository tasks\n\nUSAGE:\n    cargo xtask <COMMAND>\n\nCOMMANDS:\n    gen-fixtures        Generate a deterministic large-directory fixture\n    check-m1-budgets    Enforce headless 100k-entry latency budgets\n    check-m4-budgets    Compare copy performance with cp\n    help                Show this help"
    );
}

fn print_gen_fixtures_help() {
    println!(
        "Generate a deterministic fixture atomically.\n\nUSAGE:\n    cargo xtask gen-fixtures [OPTIONS]\n\nOPTIONS:\n    --root PATH                    Destination (default: target/fixtures/large-tree)\n    --files N                      File count (default: 100000)\n    --files-per-directory N        Bucket size (default: 100)\n    --payload-bytes N              Bytes per file (default: 32)"
    );
}

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dualpane_core::VPath;
use dualpane_engine::{
    ConflictPolicy, CopyMethod, JobEvent, JobState, OperationEngine, ScanOptions, TransferOptions,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct M4BenchOptions {
    pub large_file: PathBuf,
    pub small_tree: PathBuf,
    pub work_root: PathBuf,
}

impl Default for M4BenchOptions {
    fn default() -> Self {
        Self {
            large_file: PathBuf::from("target/fixtures/large-10g.bin"),
            small_tree: PathBuf::from("target/fixtures/small-50k"),
            work_root: PathBuf::from("target/m4-bench"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct M4BenchReport {
    pub large_engine: Duration,
    pub large_cp: Duration,
    pub large_scan: Duration,
    pub large_ratio: f64,
    pub large_method: CopyMethod,
    pub large_bytes_read: u64,
    pub small_engine: Duration,
    pub small_cp: Duration,
    pub small_scan: Duration,
    pub small_rate_ratio: f64,
}

impl M4BenchReport {
    pub fn enforce(self) -> Result<(), M4BudgetFailure> {
        let mut failures = Vec::new();
        if self.large_ratio > 1.05 {
            failures.push(format!(
                "large-file copy ratio {:.3} exceeds 1.050",
                self.large_ratio
            ));
        }
        if self.small_rate_ratio < 1.0 {
            failures.push(format!(
                "small-file rate ratio {:.3} is below 1.000",
                self.small_rate_ratio
            ));
        }
        if self.large_method == CopyMethod::Reflink && self.large_bytes_read != 0 {
            failures.push(format!(
                "reflink reported {} userspace bytes read",
                self.large_bytes_read
            ));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(M4BudgetFailure(failures))
        }
    }
}

#[derive(Debug)]
pub struct M4BudgetFailure(Vec<String>);

impl fmt::Display for M4BudgetFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.join("; "))
    }
}

impl Error for M4BudgetFailure {}

pub fn check_m4(options: &M4BenchOptions) -> Result<M4BenchReport, Box<dyn Error>> {
    if !options.large_file.is_file() {
        return Err(format!(
            "large fixture is missing: {}; create it with `fallocate -l 10G {}`",
            options.large_file.display(),
            options.large_file.display()
        )
        .into());
    }
    if !options.small_tree.is_dir() {
        return Err(format!(
            "small fixture is missing: {}; generate it with `cargo xtask gen-fixtures --root {} \
             --files 50000 --files-per-directory 100 --payload-bytes 4096`",
            options.small_tree.display(),
            options.small_tree.display()
        )
        .into());
    }
    fs::create_dir_all(&options.work_root)?;
    const SAMPLES: usize = 3;
    let mut destinations = Vec::with_capacity(SAMPLES * 4);
    for sample in 0..SAMPLES {
        destinations.extend([
            options.work_root.join(format!("large-cp-{sample}")),
            options.work_root.join(format!("large-engine-{sample}")),
            options.work_root.join(format!("small-cp-{sample}")),
            options.work_root.join(format!("small-engine-{sample}")),
        ]);
    }
    for destination in &destinations {
        remove_if_present(destination)?;
    }

    let measured = (|| {
        let mut large_cp_samples = Vec::with_capacity(SAMPLES);
        let mut large_engine_samples = Vec::with_capacity(SAMPLES);
        let mut small_cp_samples = Vec::with_capacity(SAMPLES);
        let mut small_engine_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let base = sample * 4;
            if sample % 2 == 0 {
                large_cp_samples.push(cp_once(&options.large_file, &destinations[base], false)?.0);
                large_engine_samples
                    .push(engine_once(&options.large_file, &destinations[base + 1])?);
            } else {
                large_engine_samples
                    .push(engine_once(&options.large_file, &destinations[base + 1])?);
                large_cp_samples.push(cp_once(&options.large_file, &destinations[base], false)?.0);
            }
        }
        let large_cp = median_duration(&mut large_cp_samples);
        let (large_engine, large_scan, large_outcome) =
            median_engine_sample(&mut large_engine_samples);
        let large_method = large_outcome
            .methods
            .iter()
            .max_by_key(|(_, count)| **count)
            .map_or(CopyMethod::Buffered, |(method, _)| *method);

        for sample in 0..SAMPLES {
            let base = sample * 4;
            if sample % 2 == 0 {
                small_cp_samples
                    .push(cp_once(&options.small_tree, &destinations[base + 2], true)?.0);
                small_engine_samples
                    .push(engine_once(&options.small_tree, &destinations[base + 3])?);
            } else {
                small_engine_samples
                    .push(engine_once(&options.small_tree, &destinations[base + 3])?);
                small_cp_samples
                    .push(cp_once(&options.small_tree, &destinations[base + 2], true)?.0);
            }
        }
        let small_cp = median_duration(&mut small_cp_samples);
        let (small_engine, small_scan, _) = median_engine_sample(&mut small_engine_samples);

        Ok::<_, Box<dyn Error>>(M4BenchReport {
            large_engine,
            large_cp,
            large_scan,
            large_ratio: duration_ratio(large_engine, large_cp),
            large_method,
            large_bytes_read: large_outcome.bytes_read,
            small_engine,
            small_cp,
            small_scan,
            small_rate_ratio: duration_ratio(small_cp, small_engine),
        })
    })();
    let mut cleanup_error = None;
    for destination in &destinations {
        if let Err(error) = remove_if_present(destination) {
            cleanup_error.get_or_insert(error);
        }
    }
    match (measured, cleanup_error) {
        (Ok(report), None) => Ok(report),
        (Ok(_), Some(error)) => Err(error.into()),
        (Err(error), _) => Err(error),
    }
}

fn median_duration(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn median_engine_sample(
    samples: &mut [(Duration, Duration, dualpane_engine::TransferOutcome)],
) -> (Duration, Duration, dualpane_engine::TransferOutcome) {
    samples.sort_unstable_by_key(|sample| sample.0);
    samples[samples.len() / 2].clone()
}

fn engine_once(
    source: &Path,
    destination: &Path,
) -> Result<(Duration, Duration, dualpane_engine::TransferOutcome), Box<dyn Error>> {
    fs::create_dir(destination)?;
    let started = Instant::now();
    let handle = OperationEngine::default().spawn_copy(
        vec![VPath::from(source)],
        VPath::from(destination),
        ScanOptions::default(),
        TransferOptions {
            conflict_policy: ConflictPolicy::Overwrite,
            verify: false,
            durable: false,
            parallel: true,
        },
    );
    let mut scan_elapsed = Duration::ZERO;
    while let Ok(event) = handle.events().recv() {
        match event {
            JobEvent::State {
                state: JobState::Running,
                ..
            } => scan_elapsed = started.elapsed(),
            JobEvent::Finished { .. } => break,
            JobEvent::Phase { .. }
            | JobEvent::State { .. }
            | JobEvent::Progress { .. }
            | JobEvent::Conflict { .. } => {}
        }
    }
    let summary = handle.join();
    let elapsed = started.elapsed();
    if summary.state != JobState::Done || !summary.outcome.errors.is_empty() {
        return Err(format!("engine copy failed: {:?}", summary.outcome.errors).into());
    }
    Ok((elapsed, scan_elapsed, summary.outcome))
}

fn cp_once(
    source: &Path,
    destination: &Path,
    recursive: bool,
) -> Result<(Duration, ()), Box<dyn Error>> {
    fs::create_dir(destination)?;
    let started = Instant::now();
    let mut command = Command::new("cp");
    if recursive {
        command.arg("-r");
    }
    let status = command.arg("--").arg(source).arg(destination).status()?;
    let elapsed = started.elapsed();
    if !status.success() {
        return Err(format!("cp failed with {status}").into());
    }
    Ok((elapsed, ()))
}

fn remove_if_present(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn duration_ratio(numerator: Duration, denominator: Duration) -> f64 {
    numerator.as_secs_f64() / denominator.as_secs_f64().max(f64::EPSILON)
}

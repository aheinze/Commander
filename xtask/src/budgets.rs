use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dualpane_core::{CancelToken, Filter, SortSpec, VPath};
use dualpane_index::{FuzzyFilter, ListingEvent, ListingRequest, ListingTask, hydrate_metadata};
use dualpane_vfs::{LocalFs, Vfs};

const EXPECTED_ENTRIES: usize = 100_000;
const FIRST_SNAPSHOT_BUDGET: Duration = Duration::from_millis(80);
const LISTING_SORT_STAT_BUDGET: Duration = Duration::from_millis(600);
const FILTER_BUDGET: Duration = Duration::from_millis(30);

#[derive(Clone, Copy, Debug)]
pub struct M1BudgetReport {
    pub first_snapshot: Duration,
    pub listing_sort: Duration,
    pub incremental_filter: Duration,
    pub listing_sort_stat: Duration,
}

impl M1BudgetReport {
    pub fn enforce(self) -> Result<(), BudgetFailure> {
        let checks = [
            ("first snapshot", self.first_snapshot, FIRST_SNAPSHOT_BUDGET),
            (
                "complete listing, sort, and stat",
                self.listing_sort_stat,
                LISTING_SORT_STAT_BUDGET,
            ),
            ("incremental filter", self.incremental_filter, FILTER_BUDGET),
        ];
        let failures: Vec<_> = checks
            .into_iter()
            .filter(|(_, measured, budget)| measured > budget)
            .map(|(name, measured, budget)| BudgetOverrun {
                name,
                measured,
                budget,
            })
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(BudgetFailure { failures })
        }
    }
}

#[derive(Debug)]
pub struct BudgetFailure {
    failures: Vec<BudgetOverrun>,
}

impl fmt::Display for BudgetFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, failure) in self.failures.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(
                formatter,
                "{} took {:.3} ms (budget {:.3} ms)",
                failure.name,
                failure.measured.as_secs_f64() * 1_000.0,
                failure.budget.as_secs_f64() * 1_000.0
            )?;
        }
        Ok(())
    }
}

impl Error for BudgetFailure {}

#[derive(Clone, Copy, Debug)]
struct BudgetOverrun {
    name: &'static str,
    measured: Duration,
    budget: Duration,
}

pub fn check_m1(path: &VPath) -> Result<M1BudgetReport, Box<dyn Error>> {
    let vfs: Arc<dyn Vfs> = Arc::new(LocalFs);
    let request = ListingRequest::new(path.clone());
    let task = ListingTask::spawn(Arc::clone(&vfs), request)?;
    let wall_started = Instant::now();
    let (first_snapshot, listing, timings) = match task.receiver().recv()? {
        ListingEvent::Snapshot { .. } => {
            let first_snapshot = wall_started.elapsed();
            let (listing, timings) = task.wait_complete()?;
            (first_snapshot, listing, timings)
        }
        ListingEvent::Complete { listing, timings } => (wall_started.elapsed(), listing, timings),
        ListingEvent::Cancelled => return Err("M1 listing was cancelled".into()),
        ListingEvent::Failed(error) => return Err(error.into()),
    };
    task.join()?;
    if listing.len() != EXPECTED_ENTRIES {
        return Err(format!(
            "M1 fixture has {} entries; expected {EXPECTED_ENTRIES}",
            listing.len()
        )
        .into());
    }

    let metadata = hydrate_metadata(
        vfs.as_ref(),
        &listing,
        &CancelToken::new(),
        SortSpec::default(),
    )?;
    if !metadata.failures.is_empty() {
        return Err(format!(
            "{} metadata reads failed during the M1 budget check",
            metadata.failures.len()
        )
        .into());
    }

    let mut filter = FuzzyFilter::new(&listing, &Filter::default());
    filter.finish();
    let filter_started = Instant::now();
    filter.update_query("entry-9876");
    filter.finish();
    let incremental_filter = filter_started.elapsed();

    Ok(M1BudgetReport {
        first_snapshot,
        listing_sort: timings.total,
        incremental_filter,
        listing_sort_stat: timings.total + metadata.elapsed,
    })
}

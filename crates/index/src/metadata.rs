use std::ops::Range;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dualpane_core::{CancelToken, Metadata, SortSpec, VPath};
use dualpane_vfs::{Vfs, VfsError};
use rayon::prelude::*;

use crate::{IndexError, Listing};

static METADATA_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    let workers = std::thread::available_parallelism()
        .map_or(2, usize::from)
        .min(2);
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("dualpane-metadata-{index}"))
        .build()
        .expect("metadata worker pool must be constructible")
});

/// An entry whose metadata disappeared or could not be read during a batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataFailure {
    pub source_index: u32,
    pub path: VPath,
    pub message: String,
}

/// Completed metadata batch and its worker timing.
#[derive(Clone, Debug)]
pub struct MetadataResult {
    pub listing: Arc<Listing>,
    pub failures: Vec<MetadataFailure>,
    pub elapsed: Duration,
}

/// Fetches all metadata in parallel and publishes a new immutable listing snapshot.
///
/// # Errors
///
/// Returns cancellation. Per-item filesystem failures are retained in
/// [`MetadataResult::failures`] so one vanishing file does not abort the directory.
pub fn hydrate_metadata(
    vfs: &dyn Vfs,
    listing: &Arc<Listing>,
    cancel: &CancelToken,
    sort: SortSpec,
) -> Result<MetadataResult, IndexError> {
    hydrate_indices(
        vfs,
        listing,
        cancel,
        (0..u32::try_from(listing.source_len()).expect("listing index fits u32")).collect(),
        sort,
    )
}

/// Fetches a bounded source-index range for visible rows plus lookahead.
///
/// # Errors
///
/// Returns cancellation. Per-item failures are retained in the result.
pub fn hydrate_metadata_range(
    vfs: &dyn Vfs,
    listing: &Arc<Listing>,
    cancel: &CancelToken,
    range: Range<u32>,
) -> Result<MetadataResult, IndexError> {
    let end = range
        .end
        .min(u32::try_from(listing.source_len()).expect("listing index fits u32"));
    hydrate_indices(
        vfs,
        listing,
        cancel,
        (range.start.min(end)..end).collect(),
        listing.sort(),
    )
}

/// Fetches metadata for a bounded range in visible (sorted) row order.
///
/// # Errors
///
/// Returns cancellation. Per-item failures are retained in the result.
pub fn hydrate_metadata_rows(
    vfs: &dyn Vfs,
    listing: &Arc<Listing>,
    cancel: &CancelToken,
    rows: Range<u32>,
) -> Result<MetadataResult, IndexError> {
    let end = rows
        .end
        .min(u32::try_from(listing.len()).expect("listing index fits u32"));
    let source_indices = (rows.start.min(end)..end)
        .filter_map(|row| listing.source_index_at_row(row as usize))
        .collect();
    hydrate_indices(vfs, listing, cancel, source_indices, listing.sort())
}

fn hydrate_indices(
    vfs: &dyn Vfs,
    listing: &Arc<Listing>,
    cancel: &CancelToken,
    source_indices: Vec<u32>,
    sort: SortSpec,
) -> Result<MetadataResult, IndexError> {
    let metadata_span = tracing::info_span!(
        "listing.stat_batch",
        entries = source_indices.len(),
        path = %listing.parent()
    );
    let _metadata_guard = metadata_span.enter();
    cancel.check().map_err(|_| IndexError::Cancelled)?;
    let started = Instant::now();
    let results: Vec<_> = METADATA_POOL.install(|| {
        source_indices
            .into_par_iter()
            .map(|source_index| {
                if cancel.is_cancelled() {
                    return MetadataItem::Cancelled;
                }
                let entry = listing.entry(source_index).expect("source index exists");
                let path = listing.parent().join_name(entry.name());
                match vfs.stat(&path, false) {
                    Ok(metadata) => MetadataItem::Available {
                        source_index,
                        metadata,
                    },
                    Err(VfsError::Cancelled) => MetadataItem::Cancelled,
                    Err(error) => MetadataItem::Failed(MetadataFailure {
                        source_index,
                        path,
                        message: error.to_string(),
                    }),
                }
            })
            .collect()
    });

    if cancel.is_cancelled()
        || results
            .iter()
            .any(|result| matches!(result, MetadataItem::Cancelled))
    {
        return Err(IndexError::Cancelled);
    }

    let mut updates = Vec::with_capacity(results.len());
    let mut failures = Vec::new();
    for result in results {
        match result {
            MetadataItem::Available {
                source_index,
                metadata,
            } => updates.push((source_index, metadata)),
            MetadataItem::Failed(failure) => {
                failures.push(failure);
            }
            MetadataItem::Cancelled => return Err(IndexError::Cancelled),
        }
    }

    let listing = Arc::new(listing.with_metadata_updates(&updates, sort));
    Ok(MetadataResult {
        listing,
        failures,
        elapsed: started.elapsed(),
    })
}

enum MetadataItem {
    Available {
        source_index: u32,
        metadata: Metadata,
    },
    Failed(MetadataFailure),
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use dualpane_core::{CancelToken, SortKey, SortSpec, VPath};
    use dualpane_vfs::LocalFs;
    use tempfile::tempdir;

    use crate::{ListingRequest, ListingTask};

    use super::{hydrate_metadata, hydrate_metadata_rows};

    #[test]
    fn metadata_is_fetched_in_parallel_and_can_drive_sorting() {
        let fixture = tempdir().expect("fixture");
        fs::write(fixture.path().join("small"), [0_u8]).expect("small file");
        fs::write(fixture.path().join("large"), [0_u8; 32]).expect("large file");
        let task = ListingTask::spawn(
            Arc::new(LocalFs),
            ListingRequest::new(VPath::from(fixture.path())),
        )
        .expect("listing");
        let (listing, _) = task.wait_complete().expect("complete listing");
        task.join().expect("join");
        let sort = SortSpec {
            key: SortKey::Size,
            ..SortSpec::default()
        };

        let result = hydrate_metadata(&LocalFs, &listing, &CancelToken::new(), sort)
            .expect("metadata batch");

        assert!(result.failures.is_empty());
        assert_eq!(result.listing.row(0).unwrap().name(), "small");
        assert_eq!(result.listing.row(1).unwrap().name(), "large");
        let small_index = result.listing.source_index_at_row(0).unwrap();
        let large_index = result.listing.source_index_at_row(1).unwrap();
        assert_eq!(result.listing.metadata(small_index).unwrap().size, 1);
        assert_eq!(result.listing.metadata(large_index).unwrap().size, 32);
    }

    #[test]
    fn visible_row_batches_preserve_previous_metadata() {
        let fixture = tempdir().expect("fixture");
        fs::write(fixture.path().join("file2"), [0_u8; 2]).expect("file2");
        fs::write(fixture.path().join("file10"), [0_u8; 10]).expect("file10");
        fs::write(fixture.path().join("file1"), [0_u8]).expect("file1");
        let task = ListingTask::spawn(
            Arc::new(LocalFs),
            ListingRequest::new(VPath::from(fixture.path())),
        )
        .expect("listing");
        let (listing, _) = task.wait_complete().expect("complete listing");
        task.join().expect("join");

        let first = hydrate_metadata_rows(&LocalFs, &listing, &CancelToken::new(), 0..1)
            .expect("first visible batch");
        let first_source = first.listing.source_index_at_row(0).expect("first row");
        assert_eq!(first.listing.metadata(first_source).unwrap().size, 1);

        let rest = hydrate_metadata_rows(&LocalFs, &first.listing, &CancelToken::new(), 1..3)
            .expect("second visible batch");
        let sizes: Vec<_> = (0..3)
            .map(|row| {
                let source = rest.listing.source_index_at_row(row).expect("visible row");
                rest.listing.metadata(source).expect("metadata").size
            })
            .collect();
        assert_eq!(sizes, vec![1, 2, 10]);
    }

    #[test]
    fn metadata_sorting_preserves_a_filtered_visibility_set() {
        let fixture = tempdir().expect("fixture");
        fs::write(fixture.path().join("small"), [0_u8]).expect("small");
        fs::write(fixture.path().join("large"), [0_u8; 32]).expect("large");
        fs::write(fixture.path().join(".hidden"), [0_u8; 64]).expect("hidden");
        let task = ListingTask::spawn(
            Arc::new(LocalFs),
            ListingRequest::new(VPath::from(fixture.path())),
        )
        .expect("listing");
        let (listing, _) = task.wait_complete().expect("complete listing");
        task.join().expect("join");
        let visible = listing.without_hidden();
        let sort = SortSpec {
            key: SortKey::Size,
            ..SortSpec::default()
        };

        let result = hydrate_metadata(&LocalFs, &visible, &CancelToken::new(), sort)
            .expect("metadata batch");
        let names = result
            .listing
            .rows()
            .map(|entry| entry.name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(names, ["small", "large"]);
        assert_eq!(result.listing.source_len(), 3);
    }
}

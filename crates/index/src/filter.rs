use std::sync::Arc;
use std::time::{Duration, Instant};

use dualpane_core::{CancelToken, Filter};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo, Utf32String};

use crate::{IndexError, Listing};

/// Result of advancing the background fuzzy matcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterStatus {
    pub changed: bool,
    pub running: bool,
}

/// Nucleo-backed incremental matcher built once for an immutable listing.
pub struct FuzzyFilter {
    matcher: Nucleo<u32>,
    query: String,
}

/// Completed fuzzy view and worker timing.
#[derive(Clone, Debug)]
pub struct FilterResult {
    pub listing: Arc<Listing>,
    pub elapsed: Duration,
}

impl FuzzyFilter {
    /// Builds a matcher from a listing. Candidate conversion is performed once, off the UI path.
    #[must_use]
    pub fn new(listing: &Listing, filter: &Filter) -> Self {
        Self::try_new(listing, filter, &CancelToken::new())
            .expect("fresh filter cancellation token cannot be cancelled")
    }

    /// Builds a matcher while observing cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::Cancelled`] when cancellation is requested.
    pub fn try_new(
        listing: &Listing,
        filter: &Filter,
        cancel: &CancelToken,
    ) -> Result<Self, IndexError> {
        cancel.check().map_err(|_| IndexError::Cancelled)?;
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        // The app already runs each matcher on a dedicated pane worker.  A
        // second per-matcher pool sized to every CPU multiplied idle threads
        // and allocator arenas for little interactive benefit.
        let matcher = Nucleo::new(Config::DEFAULT, notify, Some(1), 1);
        let injector = matcher.injector();

        for source_index in 0..listing.source_len() {
            if source_index.is_multiple_of(256) && cancel.is_cancelled() {
                return Err(IndexError::Cancelled);
            }
            let source_index = u32::try_from(source_index).expect("listing index fits u32");
            let entry = listing.entry(source_index).expect("source index exists");
            if !filter.include_hidden && entry.name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            let candidate = entry.name().to_string_lossy();
            injector.push(source_index, |_, columns| {
                columns[0] = Utf32String::from(candidate);
            });
        }
        drop(injector);

        let mut filter_engine = Self {
            matcher,
            query: String::new(),
        };
        filter_engine.update_query(&filter.query);
        Ok(filter_engine)
    }

    /// Updates the pattern, preserving Nucleo's append optimization for type-ahead input.
    pub fn update_query(&mut self, query: &str) {
        let append = query.starts_with(&self.query);
        self.matcher
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.query.clear();
        self.query.push_str(query);
    }

    /// Advances matching for at most `timeout_ms`; callers can use zero in UI callbacks.
    pub fn tick(&mut self, timeout_ms: u64) -> FilterStatus {
        let status = self.matcher.tick(timeout_ms);
        FilterStatus {
            changed: status.changed,
            running: status.running,
        }
    }

    /// Runs the current query to completion on its worker pool.
    pub fn finish(&mut self) {
        while self.tick(10).running {
            std::thread::yield_now();
        }
    }

    /// Runs the current query to completion while observing cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::Cancelled`] when cancellation is requested.
    pub fn finish_cancellable(&mut self, cancel: &CancelToken) -> Result<(), IndexError> {
        while self.tick(2).running {
            cancel.check().map_err(|_| IndexError::Cancelled)?;
            std::thread::yield_now();
        }
        cancel.check().map_err(|_| IndexError::Cancelled)
    }

    /// Returns matching stable source indices in score order.
    #[must_use]
    pub fn matched_indices(&self) -> Vec<u32> {
        self.matcher
            .snapshot()
            .matched_items(..)
            .map(|item| *item.data)
            .collect()
    }

    /// Returns the current query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }
}

/// Produces an immutable fuzzy-scored view of a listing on a worker thread.
///
/// # Errors
///
/// Returns cancellation before publishing a partial view.
pub fn filter_listing(
    listing: &Arc<Listing>,
    filter: &Filter,
    cancel: &CancelToken,
) -> Result<FilterResult, IndexError> {
    let span = tracing::info_span!("listing.filter", query = %filter.query, entries = listing.source_len());
    let _guard = span.enter();
    let started = Instant::now();
    let mut engine = FuzzyFilter::try_new(listing, filter, cancel)?;
    engine.finish_cancellable(cancel)?;
    let row_order = engine.matched_indices();
    cancel.check().map_err(|_| IndexError::Cancelled)?;
    Ok(FilterResult {
        listing: listing.with_row_order(row_order),
        elapsed: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::Arc;

    use dualpane_core::{Entry, EntryKind, Filter, SortSpec, VPath};

    use crate::Listing;

    use super::{FuzzyFilter, filter_listing};

    #[test]
    fn fuzzy_filter_returns_incremental_score_order() {
        let entries = ["notes.txt", "readme.txt", "thread.rs", ".hidden-read"]
            .into_iter()
            .map(|name| Entry::new(OsString::from(name), EntryKind::File, None))
            .collect();
        let listing = Listing::from_entries(
            Arc::new(VPath::from("virtual")),
            entries,
            SortSpec::default(),
        );
        let mut filter = FuzzyFilter::new(
            &listing,
            &Filter {
                query: "read".to_owned(),
                include_hidden: false,
            },
        );

        filter.finish();
        let names: Vec<_> = filter
            .matched_indices()
            .into_iter()
            .map(|index| {
                listing
                    .entry(index)
                    .unwrap()
                    .name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        insta::assert_debug_snapshot!(names, @r###"
        [
            "readme.txt",
            "thread.rs",
        ]
        "###);
    }

    #[test]
    fn cancelled_filter_does_not_publish_a_view() {
        let listing = Arc::new(Listing::from_entries(
            Arc::new(VPath::from("virtual")),
            vec![Entry::new(OsString::from("one"), EntryKind::File, None)],
            SortSpec::default(),
        ));
        let cancel = dualpane_core::CancelToken::new();
        cancel.cancel();

        let result = filter_listing(
            &listing,
            &Filter {
                query: "one".to_owned(),
                include_hidden: false,
            },
            &cancel,
        );

        assert!(matches!(result, Err(crate::IndexError::Cancelled)));
    }
}

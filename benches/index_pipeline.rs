use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use dualpane_core::{CancelToken, Filter, SortSpec, VPath};
use dualpane_index::{
    FuzzyFilter, Listing, ListingEvent, ListingRequest, ListingTask, hydrate_metadata,
};
use dualpane_vfs::{LocalFs, Vfs};

fn index_pipeline(criterion: &mut Criterion) {
    let fixture = fixture_path();
    assert!(
        fixture.is_dir(),
        "generate the flat benchmark fixture first: cargo xtask gen-fixtures --root \
         target/fixtures/flat-100k --files 100000 --files-per-directory 100000"
    );
    let vfs: Arc<dyn Vfs> = Arc::new(LocalFs);
    let request = ListingRequest::new(VPath::from(fixture.as_path()));
    let complete = load_listing(Arc::clone(&vfs), request.clone());
    assert_eq!(
        complete.len(),
        100_000,
        "benchmark fixture must contain 100k entries"
    );

    let mut group = criterion.benchmark_group("index_100k");
    group.sample_size(10);

    group.bench_function("first_snapshot", |bencher| {
        bencher.iter(|| {
            let task = ListingTask::spawn(Arc::clone(&vfs), request.clone()).expect("listing task");
            match task.receiver().recv().expect("listing event") {
                ListingEvent::Snapshot { listing, .. } | ListingEvent::Complete { listing, .. } => {
                    black_box(listing.len());
                }
                ListingEvent::Cancelled => panic!("listing cancelled unexpectedly"),
                ListingEvent::Failed(error) => panic!("listing failed: {error}"),
            }
            task.cancel();
        });
    });

    group.bench_function("complete_listing_and_sort", |bencher| {
        bencher.iter(|| {
            let listing = load_listing(Arc::clone(&vfs), request.clone());
            black_box(listing.len());
        });
    });

    group.bench_function("natural_resort", |bencher| {
        bencher.iter(|| black_box(complete.resort(SortSpec::default()).len()));
    });

    group.bench_function("incremental_filter", |bencher| {
        bencher.iter_batched(
            || FuzzyFilter::new(&complete, &Filter::default()),
            |mut filter| {
                filter.update_query("entry-9876");
                filter.finish();
                black_box(filter.matched_indices().len());
            },
            BatchSize::LargeInput,
        );
    });

    group.bench_function("full_metadata_batch", |bencher| {
        bencher.iter(|| {
            let result = hydrate_metadata(
                vfs.as_ref(),
                &complete,
                &CancelToken::new(),
                SortSpec::default(),
            )
            .expect("metadata batch");
            black_box(result.listing.len());
        });
    });

    group.bench_function("complete_listing_sort_and_stat", |bencher| {
        bencher.iter(|| {
            let listing = load_listing(Arc::clone(&vfs), request.clone());
            let result = hydrate_metadata(
                vfs.as_ref(),
                &listing,
                &CancelToken::new(),
                SortSpec::default(),
            )
            .expect("metadata batch");
            black_box(result.listing.len());
        });
    });

    group.finish();
}

fn load_listing(vfs: Arc<dyn Vfs>, request: ListingRequest) -> Arc<Listing> {
    let task = ListingTask::spawn(vfs, request).expect("listing task");
    let (listing, _) = task.wait_complete().expect("complete listing");
    task.join().expect("join listing task");
    listing
}

fn fixture_path() -> PathBuf {
    std::env::var_os("DUALPANE_BENCH_FIXTURE").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target/fixtures/flat-100k/bucket-000000")
        },
        PathBuf::from,
    )
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = index_pipeline
}
criterion_main!(benches);

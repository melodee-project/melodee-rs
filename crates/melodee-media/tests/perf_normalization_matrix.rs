//! Opt-in production normalization-path codec/concurrency measurements.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{path::PathBuf, sync::mpsc, time::Instant};

use melodee_media::{InspectionOptions, NormalizationOptions, inspect_release, normalize_release};
use uuid::Uuid;

#[test]
#[ignore = "requires generated audio fixtures and external media tools"]
fn codec_worker_matrix() {
    if cfg!(debug_assertions) {
        panic!("run this benchmark in release mode");
    }
    let source = PathBuf::from(std::env::var_os("MELODEE_PERF_SOURCE").expect("fixture source"));
    let work = PathBuf::from(std::env::var_os("MELODEE_PERF_WORK").expect("owned work area"));
    let workers: usize = std::env::var("MELODEE_PERF_WORKERS")
        .unwrap()
        .parse()
        .unwrap();
    let releases: usize = std::env::var("MELODEE_PERF_RELEASES")
        .unwrap()
        .parse()
        .unwrap();
    assert!(workers > 0 && workers <= releases);
    let mut tasks = (0..workers).map(|_| Vec::new()).collect::<Vec<_>>();
    for index in 0..releases {
        let inspection = inspect_release(
            Uuid::now_v7(),
            None,
            "benchmark",
            &source,
            None,
            Some("benchmark"),
            &InspectionOptions::default(),
        )
        .expect("inspect fixture");
        assert!(inspection.structurally_valid, "fixture must be valid");
        tasks[index % workers].push(inspection.manifest);
    }
    let (sender, receiver) = mpsc::channel();
    let start = Instant::now();
    std::thread::scope(|scope| {
        for group in tasks {
            let sender = sender.clone();
            let source = &source;
            let work = &work;
            scope.spawn(move || {
                for mut manifest in group {
                    let options = NormalizationOptions {
                        work_area: work.clone(),
                        ..NormalizationOptions::default()
                    };
                    let started = Instant::now();
                    let result = normalize_release(&mut manifest, source, &options)
                        .expect("normalize fixture");
                    assert_eq!(result.outputs_produced, 1);
                    assert_eq!(result.fingerprints_produced, 1);
                    sender
                        .send((started.elapsed().as_micros(), result.bytes_processed))
                        .unwrap();
                }
            });
        }
    });
    let elapsed = start.elapsed();
    drop(sender);
    let samples = receiver.into_iter().collect::<Vec<_>>();
    assert_eq!(samples.len(), releases);
    let durations = samples
        .iter()
        .map(|(micros, _)| *micros)
        .collect::<Vec<_>>();
    let bytes = samples.iter().map(|(_, bytes)| *bytes).sum::<u64>();
    let result = serde_json::json!({
        "workers": workers, "releases": releases, "elapsed_seconds": elapsed.as_secs_f64(),
        "releases_per_second": releases as f64 / elapsed.as_secs_f64(),
        "latency_microseconds": durations, "source_bytes_processed": bytes,
    });
    println!("MELODEE_CODEC_MATRIX_RESULT={result}");
}

use observatory_usage_core::scan_jobs::ScanJobs;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn retries_and_concurrent_requests_share_one_scan() {
    let jobs = ScanJobs::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let count = calls.clone();
    let first = jobs
        .start("a", "key", move || async move {
            count.fetch_add(1, Ordering::SeqCst);
            let _ = rx.await;
            Ok((3, vec![]))
        })
        .unwrap();
    let retry = jobs
        .start("a", "key", || async { panic!("retry must not start") })
        .unwrap();
    let simultaneous = jobs
        .start("b", "other", || async { panic!("must coalesce") })
        .unwrap();
    assert_eq!(first.job_id, retry.job_id);
    assert_eq!(first.job_id, simultaneous.job_id);
    assert!(jobs.get("other", "key").is_none());
    tx.send(()).unwrap();
    for _ in 0..100 {
        if jobs.get("a", "key").unwrap().state == "completed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(jobs.get("b", "other").unwrap().state, "completed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn partial_failures_and_panics_are_not_successes() {
    let jobs = ScanJobs::default();
    jobs.start("a", "partial", || async {
        Ok((0, vec!["failed source".into()]))
    })
    .unwrap();
    for _ in 0..100 {
        if jobs.get("a", "partial").unwrap().state != "scanning" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(jobs.get("a", "partial").unwrap().state, "partial_failure");
    jobs.start("a", "panic", || async { panic!("fixture") })
        .unwrap();
    for _ in 0..100 {
        if jobs.get("a", "panic").unwrap().state != "scanning" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(jobs.get("a", "panic").unwrap().state, "failed");
}

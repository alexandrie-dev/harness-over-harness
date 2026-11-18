use crate::{compression_limiter::*, encoding::*};
use futures_util::{
    FutureExt,
    future::{BoxFuture, join_all},
};
use hoh_error::BoxError;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
#[derive(Debug)]
struct Candidate {
    data: Vec<u8>,
    quality: u8,
}
impl EncodedCandidate for Candidate {
    fn data(&self) -> &[u8] {
        &self.data
    }
}
fn attempt(
    length: usize,
    quality: u8,
    calls: &Arc<Mutex<Vec<u8>>>,
) -> EncodingAttempt<'static, Candidate> {
    let calls = calls.clone();
    Box::new(move || {
        Box::pin(async move {
            calls.lock().unwrap().push(quality);
            Ok(Candidate {
                data: vec![0; length],
                quality,
            })
        }) as BoxFuture<'static, Result<Candidate, BoxError>>
    })
}
#[tokio::test]
async fn does_not_execute_fallback_qualities_after_the_first_fitting_candidate() {
    let calls = Arc::new(Mutex::new(vec![]));
    let result = encode_first_within_limit(vec![attempt(8, 85, &calls), attempt(4, 80, &calls)], 8)
        .await
        .unwrap()
        .into_chosen();
    assert_eq!(result.quality, 85);
    assert_eq!(*calls.lock().unwrap(), [85]);
}
#[tokio::test]
async fn executes_later_candidates_only_after_earlier_candidates_exceed_the_cap() {
    let calls = Arc::new(Mutex::new(vec![]));
    let result = encode_first_within_limit(
        vec![
            attempt(12, 85, &calls),
            attempt(7, 80, &calls),
            attempt(5, 75, &calls),
        ],
        8,
    )
    .await
    .unwrap()
    .into_chosen();
    assert_eq!(result.quality, 80);
    assert_eq!(*calls.lock().unwrap(), [85, 80]);
}
#[tokio::test]
async fn rejects_an_empty_candidate_list_and_reports_the_smallest_exhausted_candidate() {
    assert_eq!(
        encode_first_within_limit::<Candidate>(vec![], 8)
            .await
            .unwrap_err()
            .to_string(),
        "image encoding requires at least one candidate"
    );
    let calls = Arc::new(Mutex::new(vec![]));
    let result = encode_first_within_limit(
        vec![
            attempt(12, 85, &calls),
            attempt(9, 80, &calls),
            attempt(10, 75, &calls),
        ],
        8,
    )
    .await
    .unwrap();
    assert!(is_exhausted_encoding(&result));
    assert_eq!(result.into_chosen().quality, 80);
    assert!(!is_exhausted_encoding(&EncodingResult::Fitting(
        Candidate {
            data: vec![0],
            quality: 85
        }
    )));
}
#[tokio::test]
async fn starts_at_most_the_configured_number_of_tasks_and_preserves_queued_progress() {
    let limiter = CompressionLimiter::new(2);
    let started = Arc::new(Mutex::new(vec![]));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let mut gates = vec![];
    let mut tasks = vec![];
    for index in 0..4 {
        let (send, receive) = tokio::sync::oneshot::channel();
        gates.push(Some(send));
        let started = started.clone();
        let active = active.clone();
        let maximum = maximum.clone();
        let limiter = limiter.clone();
        tasks.push(async move {
            limiter
                .run(async move || {
                    maximum.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    started.lock().unwrap().push(index);
                    receive.await?;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(index)
                })
                .await
        });
    }
    let mut tasks = std::pin::pin!(join_all(tasks));
    assert!(tasks.as_mut().now_or_never().is_none());
    assert_eq!(*started.lock().unwrap(), [0, 1]);
    gates[0].take().unwrap().send(()).unwrap();
    assert!(tasks.as_mut().now_or_never().is_none());
    assert_eq!(*started.lock().unwrap(), [0, 1, 2]);
    gates[1].take().unwrap().send(()).unwrap();
    gates[2].take().unwrap().send(()).unwrap();
    assert!(tasks.as_mut().now_or_never().is_none());
    assert_eq!(*started.lock().unwrap(), [0, 1, 2, 3]);
    gates[3].take().unwrap().send(()).unwrap();
    assert_eq!(
        tasks
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert_eq!(maximum.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn releases_a_slot_when_a_task_throws_before_returning_a_promise() {
    let limiter = CompressionLimiter::new(1);
    let failed = limiter.run(async || {
        Err::<(), BoxError>(std::io::Error::other("synchronous setup failure").into())
    });
    let next = limiter.run(async || Ok("next"));
    let (failed, next) = tokio::join!(failed, next);
    assert_eq!(failed.unwrap_err().to_string(), "synchronous setup failure");
    assert_eq!(next.unwrap(), "next");
}
#[tokio::test]
async fn normalizes_a_non_error_rejection_and_releases_its_slot() {
    let limiter = CompressionLimiter::new(1);
    let failed = limiter.run(async || {
        Err::<(), BoxError>(
            compression_failure(CompressionRejection::Value(serde_json::json!(
                "native failure"
            )))
            .into(),
        )
    });
    let next = limiter.run(async || Ok("next"));
    let (failed, next) = tokio::join!(failed, next);
    let error = failed.unwrap_err();
    let shared = hoh_error::SharedError::of(error.as_ref()).unwrap();
    let error = shared.downcast_ref::<NonErrorCompressionFailure>().unwrap();
    assert_eq!(
        error.to_string(),
        "Image compression task rejected with a non-Error value."
    );
    assert_eq!(error.cause, "native failure");
    assert_eq!(next.unwrap(), "next");
}

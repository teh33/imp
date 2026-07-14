use std::pin::Pin;
use std::time::Duration;

use futures::StreamExt;
use imp_llm::{provider::RetryPolicy, StreamEvent};

/// Determines whether an `imp_llm::Error` is transient and worth retrying.
///
/// Non-retryable errors (auth failures, bad requests) should propagate
/// immediately — retrying them wastes time and may make things worse.
pub fn is_retryable(err: &imp_llm::Error) -> bool {
    match err {
        // Rate limit — always retry; the provider says to wait.
        imp_llm::Error::RateLimited { .. } => true,
        // HTTP transport/body failures: check what kind of reqwest error it is.
        // `bytes_stream()` surfaces truncated or malformed compressed response
        // bodies as decode errors (for example: "error decoding response body").
        // Those are usually transient provider/proxy failures and are safe to
        // retry before any meaningful stream event has been emitted.
        imp_llm::Error::Http(e) => {
            e.is_connect() || e.is_timeout() || e.is_request() || e.is_decode() || e.is_body()
        }
        // Stream errors are transient (connection reset, partial read, etc.).
        imp_llm::Error::Stream(_) => true,
        // Provider errors may carry an HTTP status or a machine-readable code.
        // Some streaming APIs report failures in-band without exposing the HTTP
        // status, so classify known transient codes as well.
        imp_llm::Error::Provider(msg) => is_retryable_provider_message(msg),
        // Auth errors (401, 403) and bad request (400) are permanent.
        imp_llm::Error::Auth(_) => false,
        // Serialization, IO, context-too-long: not transient.
        imp_llm::Error::Serialization(_)
        | imp_llm::Error::Io(_)
        | imp_llm::Error::ContextTooLong { .. } => false,
    }
}

fn is_retryable_provider_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "http 500",
        "http 502",
        "http 503",
        "http 504",
        "http 529",
        "server_error",
        "internal_error",
        "service_unavailable",
        "overloaded_error",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn retryable_terminal_error(event: &StreamEvent) -> Option<imp_llm::Error> {
    let StreamEvent::MessageEnd { message } = event else {
        return None;
    };
    let imp_llm::StopReason::Error(error) = &message.stop_reason else {
        return None;
    };
    is_retryable_provider_message(error).then(|| imp_llm::Error::Provider(error.clone()))
}

/// Compute how long to wait before a retry attempt.
///
/// Uses exponential backoff with random jitter in [0, base_delay / 2).
/// If the error carries a `Retry-After` hint that is within `max_delay`,
/// that takes precedence.
pub fn backoff_delay(
    attempt: u32,
    policy: &RetryPolicy,
    retry_after_secs: Option<u64>,
) -> Option<Duration> {
    // If the provider told us exactly when to retry, respect it — unless it
    // exceeds our maximum, in which case we give up immediately.
    if let Some(secs) = retry_after_secs {
        let suggested = Duration::from_secs(secs);
        if suggested > policy.max_delay {
            return None; // signal: abort, don't retry
        }
        return Some(suggested);
    }

    // Exponential backoff: base * 2^attempt, capped at max_delay.
    let base_ms = policy.base_delay.as_millis() as u64;
    let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(10));
    let capped_ms = exp_ms.min(policy.max_delay.as_millis() as u64);

    // Jitter: add up to 50% of the capped delay to spread retries.
    // Use timestamp + attempt for cheap pseudo-randomness without a rand dependency.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ (attempt as u64).wrapping_mul(0x517cc1b727220a95);
    let jitter_ms = seed % (capped_ms / 2 + 1);

    Some(Duration::from_millis(capped_ms + jitter_ms))
}

/// Stream an LLM call with automatic retry on transient startup errors.
///
/// This preserves true streaming semantics: successful events are forwarded to
/// the caller as soon as they arrive.
///
/// Retry is only transparent before the first meaningful event is emitted.
/// Leading `MessageStart` events are buffered so we can still retry if the
/// connection dies before the first text delta / tool call / completed message.
/// Once any non-`MessageStart` event is forwarded, further errors are surfaced
/// directly instead of replaying the stream.
pub fn stream_with_retry<F, S>(
    mut make_stream: F,
    policy: RetryPolicy,
) -> Pin<Box<dyn futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Send>>
where
    F: FnMut() -> S + Send + 'static,
    S: futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Unpin + Send + 'static,
{
    let (tx, rx) = futures::channel::mpsc::unbounded();

    tokio::spawn(async move {
        let mut attempt = 0u32;

        'attempt: loop {
            let mut stream = make_stream();
            let mut buffered_starts: Vec<StreamEvent> = Vec::new();
            let mut emitted_meaningful_event = false;

            while let Some(item) = stream.next().await {
                match item {
                    Ok(event) => {
                        if !emitted_meaningful_event {
                            if let Some(err) = retryable_terminal_error(&event) {
                                if attempt < policy.max_retries {
                                    match backoff_delay(attempt, &policy, None) {
                                        Some(delay) => {
                                            tokio::time::sleep(delay).await;
                                            attempt += 1;
                                            continue 'attempt;
                                        }
                                        None => {
                                            let _ = tx.unbounded_send(Err(err));
                                            return;
                                        }
                                    }
                                }
                                let _ = tx.unbounded_send(Err(err));
                                return;
                            }
                        }

                        if !emitted_meaningful_event
                            && matches!(event, StreamEvent::MessageStart { .. })
                        {
                            buffered_starts.push(event);
                            continue;
                        }

                        if !emitted_meaningful_event {
                            emitted_meaningful_event = true;
                            for buffered in buffered_starts.drain(..) {
                                if tx.unbounded_send(Ok(buffered)).is_err() {
                                    return;
                                }
                            }
                        }

                        if tx.unbounded_send(Ok(event)).is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        let retry_after =
                            if let imp_llm::Error::RateLimited { retry_after_secs } = &err {
                                *retry_after_secs
                            } else {
                                None
                            };

                        if !emitted_meaningful_event
                            && is_retryable(&err)
                            && attempt < policy.max_retries
                        {
                            match backoff_delay(attempt, &policy, retry_after) {
                                None => {
                                    let _ = tx.unbounded_send(Err(err));
                                    return;
                                }
                                Some(delay) => {
                                    tokio::time::sleep(delay).await;
                                    attempt += 1;
                                    continue 'attempt;
                                }
                            }
                        }

                        let _ = tx.unbounded_send(Err(err));
                        return;
                    }
                }
            }

            for buffered in buffered_starts {
                if tx.unbounded_send(Ok(buffered)).is_err() {
                    return;
                }
            }

            return;
        }
    });

    Box::pin(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imp_llm::provider::RetryCondition;

    fn default_policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(10), // fast for tests
            max_delay: Duration::from_millis(100),
            retry_on: vec![
                RetryCondition::RateLimit,
                RetryCondition::ServerError,
                RetryCondition::Timeout,
                RetryCondition::ConnectionError,
            ],
        }
    }

    // ── is_retryable ──────────────────────────────────────────────

    #[test]
    fn rate_limited_is_retryable() {
        let err = imp_llm::Error::RateLimited {
            retry_after_secs: Some(5),
        };
        assert!(is_retryable(&err));
    }

    #[test]
    fn stream_error_is_retryable() {
        let err = imp_llm::Error::Stream("connection reset".into());
        assert!(is_retryable(&err));
    }

    #[tokio::test]
    async fn http_decode_error_is_retryable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request_buf = [0u8; 1024];
            let _ = socket.read(&mut request_buf).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 999\r\n\r\nnot-g")
                .await
                .unwrap();
        });

        let err = reqwest::get(format!("http://{addr}"))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap_err();

        assert!(err.is_decode() || err.is_body());
        assert!(is_retryable(&imp_llm::Error::Http(err)));
    }

    #[test]
    fn auth_error_is_not_retryable() {
        let err = imp_llm::Error::Auth("invalid key".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn provider_5xx_is_retryable() {
        let err = imp_llm::Error::Provider("HTTP 503: overloaded".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn provider_server_error_code_is_retryable() {
        let err =
            imp_llm::Error::Provider("server_error: request failed; request ID req-123".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn provider_4xx_is_not_retryable() {
        let err = imp_llm::Error::Provider("HTTP 400: bad request".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn provider_401_is_not_retryable() {
        let err = imp_llm::Error::Provider("HTTP 401: unauthorized".into());
        assert!(!is_retryable(&err));
    }

    #[tokio::test]
    async fn retries_in_band_server_error_before_output() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempt_counter = attempts.clone();
        let stream = stream_with_retry(
            move || {
                let attempt = attempt_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let events = if attempt == 0 {
                    vec![StreamEvent::MessageEnd {
                        message: imp_llm::AssistantMessage {
                            content: vec![],
                            usage: None,
                            stop_reason: imp_llm::StopReason::Error(
                                "server_error: temporary failure; request ID req-123".into(),
                            ),
                            timestamp: 0,
                        },
                    }]
                } else {
                    vec![StreamEvent::TextDelta { text: "ok".into() }]
                };
                futures::stream::iter(events.into_iter().map(Ok))
            },
            RetryPolicy {
                max_retries: 1,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                retry_on: vec![],
            },
        );

        let events: Vec<_> = stream.collect::<Vec<_>>().await;
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(matches!(
            events.as_slice(),
            [Ok(StreamEvent::TextDelta { text })] if text == "ok"
        ));
    }

    #[tokio::test]
    async fn does_not_retry_in_band_server_error_after_output() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempt_counter = attempts.clone();
        let stream = stream_with_retry(
            move || {
                attempt_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                futures::stream::iter(vec![
                    Ok(StreamEvent::TextDelta {
                        text: "partial".into(),
                    }),
                    Ok(StreamEvent::MessageEnd {
                        message: imp_llm::AssistantMessage {
                            content: vec![],
                            usage: None,
                            stop_reason: imp_llm::StopReason::Error(
                                "server_error: temporary failure".into(),
                            ),
                            timestamp: 0,
                        },
                    }),
                ])
            },
            RetryPolicy {
                max_retries: 3,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                retry_on: vec![],
            },
        );

        let events: Vec<_> = stream.collect::<Vec<_>>().await;
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(events.len(), 2);
    }

    // ── backoff_delay ─────────────────────────────────────────────

    #[test]
    fn backoff_grows_exponentially() {
        let policy = default_policy();
        let d0 = backoff_delay(0, &policy, None).unwrap();
        let d1 = backoff_delay(1, &policy, None).unwrap();
        let d2 = backoff_delay(2, &policy, None).unwrap();
        // Each step should generally be larger (accounting for jitter).
        // At minimum: base*2^0=10ms, base*2^1=20ms, base*2^2=40ms
        // With jitter added, d1 >= 20ms and d2 >= 40ms (before jitter on d0).
        // We can only assert upper bounds reliably given jitter.
        assert!(d0 <= Duration::from_millis(200)); // 10ms base + 50% jitter, capped
        assert!(d1 >= Duration::from_millis(20));
        assert!(d2 >= Duration::from_millis(40));
    }

    #[test]
    fn backoff_capped_at_max_delay() {
        let policy = default_policy(); // max 100ms
                                       // Attempt 10 would be base(10ms) * 2^10 = 10_240ms → capped at 100ms
        let delay = backoff_delay(10, &policy, None).unwrap();
        assert!(delay <= Duration::from_millis(200)); // cap + up to 50% jitter of cap
    }

    #[test]
    fn retry_after_respected_within_limit() {
        let policy = default_policy(); // max 100ms
        let delay = backoff_delay(0, &policy, Some(0)).unwrap();
        assert_eq!(delay, Duration::from_secs(0));
    }

    #[test]
    fn retry_after_exceeds_max_delay_returns_none() {
        let policy = default_policy(); // max 100ms
        let result = backoff_delay(0, &policy, Some(10)); // 10s > 100ms
        assert!(result.is_none());
    }

    // ── stream_with_retry ────────────────────────────────────────

    #[tokio::test]
    async fn retry_succeeds_after_transient_failures_before_first_meaningful_event() {
        use std::sync::{Arc, Mutex};

        let call_count = Arc::new(Mutex::new(0u32));

        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(50),
            retry_on: vec![RetryCondition::ServerError],
        };

        let call_count_clone = call_count.clone();
        let mut stream = stream_with_retry(
            move || {
                let mut count = call_count_clone.lock().unwrap();
                *count += 1;
                let attempt = *count;
                drop(count);

                if attempt < 3 {
                    let events: Vec<imp_llm::Result<StreamEvent>> = vec![
                        Ok(StreamEvent::MessageStart {
                            model: "test".into(),
                        }),
                        Err(imp_llm::Error::Stream("transient".into())),
                    ];
                    futures::stream::iter(events)
                } else {
                    let events: Vec<imp_llm::Result<StreamEvent>> = vec![
                        Ok(StreamEvent::MessageStart {
                            model: "test".into(),
                        }),
                        Ok(StreamEvent::TextDelta {
                            text: "hello".into(),
                        }),
                    ];
                    futures::stream::iter(events)
                }
            },
            policy,
        );

        let mut result = Vec::new();
        while let Some(item) = stream.next().await {
            result.push(item);
        }

        assert_eq!(*call_count.lock().unwrap(), 3);
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], Ok(StreamEvent::MessageStart { .. })));
        assert!(matches!(result[1], Ok(StreamEvent::TextDelta { .. })));
    }

    #[tokio::test]
    async fn retry_exhausts_max_retries_before_first_meaningful_event() {
        use std::sync::{Arc, Mutex};

        let call_count = Arc::new(Mutex::new(0u32));

        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(50),
            retry_on: vec![RetryCondition::ServerError],
        };

        let call_count_clone = call_count.clone();
        let mut stream = stream_with_retry(
            move || {
                *call_count_clone.lock().unwrap() += 1;
                let events: Vec<imp_llm::Result<StreamEvent>> =
                    vec![Err(imp_llm::Error::Stream("always fails".into()))];
                futures::stream::iter(events)
            },
            policy,
        );

        let mut result = Vec::new();
        while let Some(item) = stream.next().await {
            result.push(item);
        }

        assert_eq!(*call_count.lock().unwrap(), 3);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], Err(imp_llm::Error::Stream(_))));
    }

    #[tokio::test]
    async fn retry_skips_non_retryable_errors() {
        use std::sync::{Arc, Mutex};

        let call_count = Arc::new(Mutex::new(0u32));

        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(50),
            retry_on: vec![RetryCondition::ServerError],
        };

        let call_count_clone = call_count.clone();
        let mut stream = stream_with_retry(
            move || {
                *call_count_clone.lock().unwrap() += 1;
                let events: Vec<imp_llm::Result<StreamEvent>> =
                    vec![Err(imp_llm::Error::Auth("invalid key".into()))];
                futures::stream::iter(events)
            },
            policy,
        );

        let mut result = Vec::new();
        while let Some(item) = stream.next().await {
            result.push(item);
        }

        assert_eq!(*call_count.lock().unwrap(), 1);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], Err(imp_llm::Error::Auth(_))));
    }

    #[tokio::test]
    async fn retry_no_error_passes_through() {
        let policy = default_policy();

        let mut stream = stream_with_retry(
            || {
                let events: Vec<imp_llm::Result<StreamEvent>> = vec![
                    Ok(StreamEvent::MessageStart {
                        model: "test".into(),
                    }),
                    Ok(StreamEvent::TextDelta { text: "ok".into() }),
                ];
                futures::stream::iter(events)
            },
            policy,
        );

        let mut result = Vec::new();
        while let Some(item) = stream.next().await {
            result.push(item);
        }

        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn retry_does_not_replay_after_meaningful_event_has_streamed() {
        use std::sync::{Arc, Mutex};

        let call_count = Arc::new(Mutex::new(0u32));
        let policy = default_policy();
        let call_count_clone = call_count.clone();

        let mut stream = stream_with_retry(
            move || {
                *call_count_clone.lock().unwrap() += 1;
                let events: Vec<imp_llm::Result<StreamEvent>> = vec![
                    Ok(StreamEvent::TextDelta {
                        text: "partial".into(),
                    }),
                    Err(imp_llm::Error::Stream("boom".into())),
                ];
                futures::stream::iter(events)
            },
            policy,
        );

        let mut result = Vec::new();
        while let Some(item) = stream.next().await {
            result.push(item);
        }

        assert_eq!(*call_count.lock().unwrap(), 1);
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], Ok(StreamEvent::TextDelta { .. })));
        assert!(matches!(result[1], Err(imp_llm::Error::Stream(_))));
    }
}

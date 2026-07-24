use std::io;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use taskcast_core::{
    DependencyErrorKind, DependencyName, DependencyObservation, DependencyObservationState,
    DependencyObserver,
};
use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use crate::broadcast::{dispatch_message, HandlerMap};
use crate::connection::classify_redis_error;

pub struct RedisPubSubHandle {
    shutdown_tx: watch::Sender<bool>,
    status_rx: watch::Receiver<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl RedisPubSubHandle {
    pub(crate) fn start(
        client: redis::Client,
        channel_prefix: String,
        handlers: HandlerMap,
        observer: Option<Arc<dyn DependencyObserver>>,
    ) -> (Self, oneshot::Receiver<()>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (status_tx, status_rx) = watch::channel(false);
        let (initialized_tx, initialized_rx) = oneshot::channel();
        let task = tokio::spawn(run_supervisor(
            client,
            channel_prefix,
            handlers,
            observer,
            shutdown_rx,
            status_tx,
            initialized_tx,
        ));

        (
            Self {
                shutdown_tx,
                status_rx,
                task: Mutex::new(Some(task)),
            },
            initialized_rx,
        )
    }

    pub fn is_subscribed(&self) -> bool {
        *self.status_rx.borrow()
    }

    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

impl Drop for RedisPubSubHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.get_mut().take() {
            task.abort();
        }
    }
}

async fn run_supervisor(
    client: redis::Client,
    channel_prefix: String,
    handlers: HandlerMap,
    observer: Option<Arc<dyn DependencyObserver>>,
    mut shutdown_rx: watch::Receiver<bool>,
    status_tx: watch::Sender<bool>,
    initialized_tx: oneshot::Sender<()>,
) {
    let pattern = format!("{channel_prefix}*");
    let mut initialized_tx = Some(initialized_tx);
    let mut attempt = 0_u32;

    loop {
        if shutdown_requested(&shutdown_rx) {
            break;
        }

        let connect_result = tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown_rx) => break,
            result = client.get_async_pubsub() => result,
        };
        let mut connection = match connect_result {
            Ok(connection) => connection,
            Err(error) => {
                let kind = classify_redis_error(&error).unwrap_or(DependencyErrorKind::Unavailable);
                if !retry_after(kind, &mut attempt, &observer, &mut shutdown_rx, &status_tx).await {
                    break;
                }
                continue;
            }
        };

        let subscribe_result = tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown_rx) => break,
            result = connection.psubscribe(&pattern) => result,
        };
        if let Err(error) = subscribe_result {
            let kind = classify_redis_error(&error).unwrap_or(DependencyErrorKind::Unavailable);
            if !retry_after(kind, &mut attempt, &observer, &mut shutdown_rx, &status_tx).await {
                break;
            }
            continue;
        }

        attempt = 0;
        let _ = status_tx.send(true);
        observe(
            &observer,
            DependencyObservationState::Healthy,
            None,
            None,
            None,
        );
        if let Some(initialized_tx) = initialized_tx.take() {
            let _ = initialized_tx.send(());
        }

        let mut messages = connection.on_message();
        let stream_ended = loop {
            let message = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => break false,
                message = messages.next() => message,
            };
            let Some(message) = message else {
                break true;
            };
            dispatch_message(&handlers, &channel_prefix, message).await;
        };
        drop(messages);
        let _ = status_tx.send(false);

        if !stream_ended {
            break;
        }
        if !retry_after(
            DependencyErrorKind::ConnectionClosed,
            &mut attempt,
            &observer,
            &mut shutdown_rx,
            &status_tx,
        )
        .await
        {
            break;
        }
    }

    let _ = status_tx.send(false);
}

async fn retry_after(
    kind: DependencyErrorKind,
    attempt: &mut u32,
    observer: &Option<Arc<dyn DependencyObserver>>,
    shutdown_rx: &mut watch::Receiver<bool>,
    status_tx: &watch::Sender<bool>,
) -> bool {
    let _ = status_tx.send(false);
    if shutdown_requested(shutdown_rx) {
        return false;
    }

    *attempt = attempt.saturating_add(1);
    let delay = equal_jitter_delay(attempt.saturating_sub(1), fastrand::f64());
    observe(
        observer,
        DependencyObservationState::Reconnecting,
        Some(kind),
        Some(*attempt),
        Some(delay.as_millis() as u64),
    );

    tokio::select! {
        biased;
        _ = wait_for_shutdown(shutdown_rx) => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn shutdown_requested(shutdown_rx: &watch::Receiver<bool>) -> bool {
    *shutdown_rx.borrow()
}

async fn wait_for_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    if shutdown_requested(shutdown_rx) {
        return;
    }
    loop {
        if shutdown_rx.changed().await.is_err() || shutdown_requested(shutdown_rx) {
            return;
        }
    }
}

fn observe(
    observer: &Option<Arc<dyn DependencyObserver>>,
    state: DependencyObservationState,
    error_kind: Option<DependencyErrorKind>,
    attempt: Option<u32>,
    next_retry_ms: Option<u64>,
) {
    let Some(observer) = observer else {
        return;
    };
    observer.observe(DependencyObservation {
        dependency: DependencyName::RedisPubSub,
        state,
        error_kind,
        attempt,
        next_retry_ms,
    });
}

pub(crate) fn equal_jitter_delay(attempt: u32, random_unit: f64) -> Duration {
    let cap_ms = (500_u64.saturating_mul(2_u64.saturating_pow(attempt))).min(10_000);
    Duration::from_millis(cap_ms / 2 + ((cap_ms as f64 / 2.0) * random_unit) as u64)
}

pub(crate) fn startup_cancelled_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "Redis PubSub startup was cancelled",
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::equal_jitter_delay;

    #[test]
    fn equal_jitter_uses_half_to_full_cap_and_caps_at_ten_seconds() {
        assert_eq!(equal_jitter_delay(0, 0.0), Duration::from_millis(250));
        assert_eq!(equal_jitter_delay(1, 1.0), Duration::from_millis(1_000));
        assert_eq!(equal_jitter_delay(20, 0.0), Duration::from_millis(5_000));
        assert_eq!(equal_jitter_delay(20, 1.0), Duration::from_millis(10_000));
    }
}

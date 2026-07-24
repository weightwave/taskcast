use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::error::{DatabaseError, ErrorKind};
use sqlx::postgres::PgPoolOptions;
use taskcast_core::{
    DependencyErrorKind, DependencyName, DependencyObservation, DependencyObservationState,
    DependencyObserver, DependencyUnavailableError, LongTermStore,
};
use taskcast_postgres::{classify_postgres_connectivity, postgres_check, PostgresLongTermStore};
use testcontainers::{runners::AsyncRunner, ImageExt};
use testcontainers_modules::postgres::Postgres;

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<DependencyObservation>>,
}

impl RecordingObserver {
    fn observations(&self) -> Vec<DependencyObservation> {
        self.observations.lock().unwrap().clone()
    }
}

impl DependencyObserver for RecordingObserver {
    fn observe(&self, observation: DependencyObservation) {
        self.observations.lock().unwrap().push(observation);
    }
}

#[derive(Debug)]
struct TestDatabaseError {
    code: &'static str,
}

impl fmt::Display for TestDatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "database error {}", self.code)
    }
}

impl Error for TestDatabaseError {}

impl DatabaseError for TestDatabaseError {
    fn message(&self) -> &str {
        "test database error"
    }

    fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.code))
    }

    fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

#[derive(Debug)]
struct WrappedSqlxError(sqlx::Error);

impl fmt::Display for WrappedSqlxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "wrapped SQLx error")
    }
}

impl Error for WrappedSqlxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

#[test]
fn classifies_sqlx_connectivity_errors_and_source_chains() {
    let cases = [
        (
            sqlx::Error::Io(io::Error::from(io::ErrorKind::ConnectionRefused)),
            DependencyErrorKind::ConnectionRefused,
        ),
        (
            sqlx::Error::Io(io::Error::from(io::ErrorKind::ConnectionReset)),
            DependencyErrorKind::ConnectionReset,
        ),
        (
            sqlx::Error::Io(io::Error::from(io::ErrorKind::TimedOut)),
            DependencyErrorKind::Timeout,
        ),
        (
            sqlx::Error::Io(io::Error::from(io::ErrorKind::NotFound)),
            DependencyErrorKind::Dns,
        ),
        (
            sqlx::Error::Io(io::Error::from(io::ErrorKind::UnexpectedEof)),
            DependencyErrorKind::ConnectionClosed,
        ),
        (sqlx::Error::PoolTimedOut, DependencyErrorKind::Timeout),
        (
            sqlx::Error::PoolClosed,
            DependencyErrorKind::ConnectionClosed,
        ),
        (sqlx::Error::WorkerCrashed, DependencyErrorKind::Unavailable),
        (
            sqlx::Error::Tls(Box::new(io::Error::other("TLS handshake failed"))),
            DependencyErrorKind::Unavailable,
        ),
        (
            sqlx::Error::Database(Box::new(TestDatabaseError { code: "08006" })),
            DependencyErrorKind::Unavailable,
        ),
        (
            sqlx::Error::Database(Box::new(TestDatabaseError { code: "57P01" })),
            DependencyErrorKind::Unavailable,
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(classify_postgres_connectivity(&error), Some(expected));
    }

    let wrapped = WrappedSqlxError(sqlx::Error::PoolClosed);
    assert_eq!(
        classify_postgres_connectivity(&wrapped),
        Some(DependencyErrorKind::ConnectionClosed)
    );
}

#[test]
fn does_not_classify_database_or_application_errors() {
    for code in ["23505", "23503", "23514", "42601", "08", "080000"] {
        let error = sqlx::Error::Database(Box::new(TestDatabaseError { code }));
        assert_eq!(classify_postgres_connectivity(&error), None);
    }

    for message in [
        "Task already exists: task-1",
        "Archive event id conflicts with another task: event-1",
        "validation failed",
    ] {
        let error = io::Error::new(io::ErrorKind::InvalidInput, message);
        assert_eq!(classify_postgres_connectivity(&error), None);
    }
}

#[tokio::test]
async fn public_operation_is_observed_once_and_reports_healthy_recovery() {
    let container = Postgres::default()
        .with_tag("16-alpine")
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(100))
        .connect(&database_url)
        .await
        .unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let store = PostgresLongTermStore::new_observed(pool.clone(), observer.clone());
    store.migrate().await.unwrap();

    let held_connection = pool.acquire().await.unwrap();
    let error = store.get_task("missing").await.unwrap_err();
    let unavailable = error
        .downcast_ref::<DependencyUnavailableError>()
        .expect("classified failures use DependencyUnavailableError");
    assert_eq!(unavailable.dependency(), DependencyName::Postgres);
    assert_eq!(unavailable.kind(), DependencyErrorKind::Timeout);
    assert!(matches!(
        unavailable
            .source()
            .and_then(|source| source.downcast_ref::<sqlx::Error>()),
        Some(sqlx::Error::PoolTimedOut)
    ));
    assert_eq!(
        observer.observations(),
        vec![DependencyObservation {
            dependency: DependencyName::Postgres,
            state: DependencyObservationState::Unhealthy,
            error_kind: Some(DependencyErrorKind::Timeout),
            attempt: None,
            next_retry_ms: None,
        }]
    );

    drop(held_connection);
    assert_eq!(store.get_task("missing").await.unwrap(), None);
    assert_eq!(
        observer.observations(),
        vec![
            DependencyObservation {
                dependency: DependencyName::Postgres,
                state: DependencyObservationState::Unhealthy,
                error_kind: Some(DependencyErrorKind::Timeout),
                attempt: None,
                next_retry_ms: None,
            },
            DependencyObservation {
                dependency: DependencyName::Postgres,
                state: DependencyObservationState::Healthy,
                error_kind: None,
                attempt: None,
                next_retry_ms: None,
            },
        ]
    );

    postgres_check(store.pool()).await.unwrap();

    sqlx::query("DROP TABLE taskcast_tasks CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    let ordinary_observer = Arc::new(RecordingObserver::default());
    let ordinary_store = PostgresLongTermStore::new_observed(pool, ordinary_observer.clone());
    let ordinary_error = ordinary_store.get_task("missing").await.unwrap_err();
    let sqlx_error = ordinary_error
        .downcast_ref::<sqlx::Error>()
        .expect("ordinary database errors are returned unchanged");
    assert_eq!(
        sqlx_error
            .as_database_error()
            .and_then(DatabaseError::code)
            .as_deref(),
        Some("42P01")
    );
    assert!(ordinary_observer.observations().is_empty());
}

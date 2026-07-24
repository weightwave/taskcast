#[path = "../../taskcast-redis/tests/support/mod.rs"]
mod support;

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::error::{DatabaseError, ErrorKind};
use sqlx::postgres::PgPoolOptions;
use support::TcpFaultProxy;
use taskcast_core::{
    DependencyErrorKind, DependencyName, DependencyObservation, DependencyObservationState,
    DependencyObserver, DependencyUnavailableError, LongTermStore, Task, TaskStatus,
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

#[derive(Debug)]
struct CyclicError;

impl fmt::Display for CyclicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cyclic error")
    }
}

impl Error for CyclicError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

fn make_recovery_task() -> Task {
    Task {
        id: "task-postgres-recovered".to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: None,
        result: None,
        error: None,
        metadata: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
        created_at: 1_000.0,
        updated_at: 1_000.0,
        completed_at: None,
        ttl: None,
    }
}

async fn eventually_postgres(pool: &sqlx::PgPool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if postgres_check(pool).await.is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "PostgreSQL pool did not recover before the deadline"
        );
        tokio::task::yield_now().await;
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

#[test]
fn classifier_returns_for_a_cyclic_source_chain() {
    let error = CyclicError;

    assert_eq!(classify_postgres_connectivity(&error), None);
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

#[tokio::test]
async fn same_pool_recovers_readiness_and_store_without_replaying_statement() {
    let container = Postgres::default()
        .with_tag("16-alpine")
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let upstream = format!("127.0.0.1:{port}").parse().unwrap();
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let database_url = format!("postgres://postgres:postgres@{}/postgres", proxy.address());
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect(&database_url)
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool.clone());
    store.migrate().await.unwrap();
    postgres_check(&pool).await.unwrap();

    let marker = "taskcast_pg_no_replay_rust";
    sqlx::query(
        "CREATE TABLE taskcast_test_no_replay (
            marker TEXT PRIMARY KEY,
            executions INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO taskcast_test_no_replay (marker, executions)
         VALUES ($1, 0)",
    )
    .bind(marker)
    .execute(&pool)
    .await
    .unwrap();
    let marker_bytes = marker.as_bytes().to_vec();
    let matched_before = proxy.matched_commands();
    proxy
        .drop_next_response(move |request| {
            request
                .windows(marker_bytes.len())
                .any(|window| window == marker_bytes)
        })
        .await;
    let statement = format!(
        "UPDATE taskcast_test_no_replay
         SET executions = executions + 1
         WHERE marker = '{marker}'
         /* {marker} */
         RETURNING executions"
    );
    let interrupted = tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::raw_sql(&statement).execute(&pool),
    )
    .await
    .expect("in-flight PostgreSQL statement did not settle before the deadline");
    assert!(
        interrupted.is_err(),
        "the statement whose response was dropped must fail"
    );
    assert_eq!(proxy.matched_commands() - matched_before, 1);

    proxy.refuse().await;
    let readiness = tokio::time::timeout(Duration::from_secs(5), postgres_check(&pool))
        .await
        .expect("PostgreSQL readiness did not settle during refusal");
    assert!(readiness.is_err(), "readiness must fail during refusal");

    proxy.open().await;
    eventually_postgres(&pool).await;
    let executions: i32 =
        sqlx::query_scalar("SELECT executions FROM taskcast_test_no_replay WHERE marker = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(executions, 1, "the interrupted statement was replayed");
    let recovered_task = make_recovery_task();
    store.save_task(recovered_task.clone()).await.unwrap();
    assert_eq!(
        store.get_task(&recovered_task.id).await.unwrap(),
        Some(recovered_task)
    );
    assert_eq!(
        proxy.matched_commands() - matched_before,
        1,
        "the interrupted statement must not be replayed"
    );

    pool.close().await;
    proxy.stop().await;
}

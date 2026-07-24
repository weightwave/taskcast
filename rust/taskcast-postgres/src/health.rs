use std::error::Error;
use std::io;

use sqlx::PgPool;
use taskcast_core::{DependencyErrorKind, DependencyName, DependencyUnavailableError};

pub fn classify_postgres_connectivity(
    mut error: &(dyn Error + 'static),
) -> Option<DependencyErrorKind> {
    // Third-party adapter errors can violate the usual acyclic source-chain
    // convention. Bound the traversal so classification cannot hang.
    for _ in 0..64 {
        if let Some(error) = error.downcast_ref::<sqlx::Error>() {
            return classify_sqlx_error(error);
        }
        error = error.source()?;
    }
    None
}

fn classify_sqlx_error(error: &sqlx::Error) -> Option<DependencyErrorKind> {
    match error {
        sqlx::Error::Io(error) => Some(classify_io_error(error)),
        sqlx::Error::PoolTimedOut => Some(DependencyErrorKind::Timeout),
        sqlx::Error::PoolClosed => Some(DependencyErrorKind::ConnectionClosed),
        sqlx::Error::WorkerCrashed | sqlx::Error::Tls(_) => Some(DependencyErrorKind::Unavailable),
        sqlx::Error::Database(error) => {
            let code = error.code()?;
            ((code.len() == 5 && code.starts_with("08")) || code == "57P01")
                .then_some(DependencyErrorKind::Unavailable)
        }
        _ => None,
    }
}

fn classify_io_error(error: &io::Error) -> DependencyErrorKind {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => DependencyErrorKind::ConnectionRefused,
        io::ErrorKind::ConnectionReset => DependencyErrorKind::ConnectionReset,
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => DependencyErrorKind::Timeout,
        io::ErrorKind::NotFound => DependencyErrorKind::Dns,
        io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof | io::ErrorKind::NotConnected => {
            DependencyErrorKind::ConnectionClosed
        }
        _ => DependencyErrorKind::Unavailable,
    }
}

pub async fn postgres_check(pool: &PgPool) -> Result<(), DependencyUnavailableError> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|error| {
            let kind =
                classify_postgres_connectivity(&error).unwrap_or(DependencyErrorKind::Unavailable);
            DependencyUnavailableError::new(DependencyName::Postgres, kind, error)
        })
}

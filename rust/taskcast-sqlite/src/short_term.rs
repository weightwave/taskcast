use sqlx::SqlitePool;

pub struct SqliteShortTermStore {
    pool: SqlitePool,
}

impl SqliteShortTermStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

// ShortTermStore trait implementation will be added in Task 7

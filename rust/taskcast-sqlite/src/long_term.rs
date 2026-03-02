use sqlx::SqlitePool;

pub struct SqliteLongTermStore {
    pool: SqlitePool,
}

impl SqliteLongTermStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

// LongTermStore trait implementation will be added in Task 8

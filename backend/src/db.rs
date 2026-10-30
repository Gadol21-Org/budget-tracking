use rusqlite::Connection;

pub fn open(path: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS categories (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             is_income INTEGER NOT NULL DEFAULT 0,
             paamonim_item_id TEXT UNIQUE,
             parent_id INTEGER REFERENCES categories(id)
         );
         CREATE TABLE IF NOT EXISTS payment_methods (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             paamonim_id TEXT UNIQUE
         );
         CREATE TABLE IF NOT EXISTS transactions (
             id INTEGER PRIMARY KEY,
             date TEXT NOT NULL,                 -- YYYY-MM-DD
             amount_agorot INTEGER NOT NULL,     -- exact, may be non-round
             category_id INTEGER REFERENCES categories(id),
             payment_method_id INTEGER REFERENCES payment_methods(id),
             note TEXT NOT NULL DEFAULT '',
             paamonim_line_id TEXT UNIQUE,       -- set once mirrored/imported
             sync_state TEXT NOT NULL DEFAULT 'pending', -- pending | synced | modified | error | local
             recurring_series_id INTEGER          -- set on a settled repeating transaction
         );
         -- Months already turned into transactions, kept even if the transaction is deleted.
         CREATE TABLE IF NOT EXISTS recurring_settled (
             series_id INTEGER NOT NULL,
             month TEXT NOT NULL,
             PRIMARY KEY (series_id, month)
         );
         -- Rules for repeating transactions; occurrences are generated, not stored.
         CREATE TABLE IF NOT EXISTS recurring (
             id INTEGER PRIMARY KEY,
             series_id INTEGER NOT NULL,         -- id of the series' first segment
             start_month TEXT NOT NULL,          -- YYYY-MM
             end_month TEXT,                     -- inclusive; NULL = forever
             day INTEGER NOT NULL,
             amount_agorot INTEGER NOT NULL,
             category_id INTEGER REFERENCES categories(id),
             payment_method_id INTEGER REFERENCES payment_methods(id),
             note TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS idx_tx_date ON transactions(date);
         CREATE INDEX IF NOT EXISTS idx_tx_category_date ON transactions(category_id, date);",
    )?;
    // Databases created before sub-categories existed.
    let has_parent = conn
        .prepare("SELECT 1 FROM pragma_table_info('categories') WHERE name = 'parent_id'")?
        .exists([])?;
    if !has_parent {
        conn.execute(
            "ALTER TABLE categories ADD COLUMN parent_id INTEGER REFERENCES categories(id)",
            [],
        )?;
    }
    let has_series = conn
        .prepare(
            "SELECT 1 FROM pragma_table_info('transactions') WHERE name = 'recurring_series_id'",
        )?
        .exists([])?;
    if !has_series {
        conn.execute(
            "ALTER TABLE transactions ADD COLUMN recurring_series_id INTEGER",
            [],
        )?;
    }
    Ok(conn)
}

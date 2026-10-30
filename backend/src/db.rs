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
             sync_state TEXT NOT NULL DEFAULT 'pending' -- pending | synced | error
         );
         CREATE INDEX IF NOT EXISTS idx_tx_date ON transactions(date);
         CREATE INDEX IF NOT EXISTS idx_tx_category_date ON transactions(category_id, date);",
    )?;
    // Databases created before sub-categories existed.
    let has_parent = conn
        .prepare("SELECT 1 FROM pragma_table_info('categories') WHERE name = 'parent_id'")?
        .exists([])?;
    if !has_parent {
        conn.execute("ALTER TABLE categories ADD COLUMN parent_id INTEGER REFERENCES categories(id)", [])?;
    }
    Ok(conn)
}

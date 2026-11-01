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
             parent_id INTEGER REFERENCES categories(id),
             -- Deleted locally. Kept (when it has a Paamonim id) so re-imports skip it;
             -- its lines go to merged_into, if any.
             deleted INTEGER NOT NULL DEFAULT 0,
             merged_into INTEGER REFERENCES categories(id),
             -- On a top-level category made here: the Paamonim top-level category it stands in for.
             linked_to INTEGER REFERENCES categories(id)
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
             sync_state TEXT NOT NULL DEFAULT 'pending', -- pending | synced | modified | verifying | error | skipped | local
             recurring_series_id INTEGER,         -- set on a settled repeating transaction
             mirror_date TEXT                     -- the line's date in Paamonim, when linked
         );
         -- Lines to delete in Paamonim: their transaction was deleted or replaced here.
         CREATE TABLE IF NOT EXISTS mirror_deletes (
             line_id TEXT PRIMARY KEY,
             date TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS mirror_meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
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
             note TEXT NOT NULL DEFAULT '',
             currency TEXT,                      -- foreign-currency rule: USD, EUR, JPY
             orig_minor INTEGER                  -- its signed amount in cents / whole yen
         );
         -- What changed, with values before and after as JSON; pruned after a year.
         CREATE TABLE IF NOT EXISTS change_log (
             id INTEGER PRIMARY KEY,
             at TEXT NOT NULL,                   -- UTC, YYYY-MM-DDTHH:MM:SSZ
             kind TEXT NOT NULL,
             entity_id INTEGER,
             old_value TEXT,
             new_value TEXT
         );
         -- Businesses (sub-businesses of one whole business) and which transactions and
         -- categories count in them. Local only: nothing here is mirrored to Paamonim.
         CREATE TABLE IF NOT EXISTS businesses (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             is_general INTEGER NOT NULL DEFAULT 0   -- shared costs not tied to one business
         );
         CREATE TABLE IF NOT EXISTS transaction_business (
             transaction_id INTEGER PRIMARY KEY,
             business_id INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS category_business (
             category_id INTEGER PRIMARY KEY,
             business_id INTEGER NOT NULL
         );
         -- Categories whose new transactions must say which business they belong to (or none).
         CREATE TABLE IF NOT EXISTS category_business_required (
             category_id INTEGER PRIMARY KEY
         );
         -- Transactions entered in a foreign currency: the original amount and the rate used for
         -- the shekel amount. Local only; Paamonim only ever gets the shekel amount.
         CREATE TABLE IF NOT EXISTS transaction_fx (
             transaction_id INTEGER PRIMARY KEY,
             currency TEXT NOT NULL,
             orig_minor INTEGER NOT NULL,
             rate REAL NOT NULL,
             rate_date TEXT NOT NULL,
             estimated INTEGER NOT NULL DEFAULT 0
         );
         -- Exchange-rate cache: shekels per unit, by day (`latest` = newest fetched).
         CREATE TABLE IF NOT EXISTS fx_rates (
             currency TEXT NOT NULL,
             date TEXT NOT NULL,
             rate REAL NOT NULL,
             PRIMARY KEY (currency, date)
         );
         CREATE INDEX IF NOT EXISTS idx_change_log_at ON change_log(at);
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
    // Databases created before categories could be deleted.
    let has_deleted = conn
        .prepare("SELECT 1 FROM pragma_table_info('categories') WHERE name = 'deleted'")?
        .exists([])?;
    if !has_deleted {
        conn.execute_batch(
            "ALTER TABLE categories ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE categories ADD COLUMN merged_into INTEGER REFERENCES categories(id);",
        )?;
    }
    // Databases created before top-level categories could be linked to Paamonim ones.
    let has_link = conn
        .prepare("SELECT 1 FROM pragma_table_info('categories') WHERE name = 'linked_to'")?
        .exists([])?;
    if !has_link {
        conn.execute(
            "ALTER TABLE categories ADD COLUMN linked_to INTEGER REFERENCES categories(id)",
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
    let has_mirror_date = conn
        .prepare("SELECT 1 FROM pragma_table_info('transactions') WHERE name = 'mirror_date'")?
        .exists([])?;
    if !has_mirror_date {
        conn.execute("ALTER TABLE transactions ADD COLUMN mirror_date TEXT", [])?;
    }
    let has_rule_currency = conn
        .prepare("SELECT 1 FROM pragma_table_info('recurring') WHERE name = 'currency'")?
        .exists([])?;
    if !has_rule_currency {
        conn.execute_batch(
            "ALTER TABLE recurring ADD COLUMN currency TEXT;
             ALTER TABLE recurring ADD COLUMN orig_minor INTEGER;",
        )?;
    }
    Ok(conn)
}

use chrono::Utc;
use rusqlite::Connection;
use std::path::Path;

pub struct Cursor {
    pub last_msg_id: i64,
    pub last_msg_date: String,
    pub last_run_at: String,
    pub total_scanned: i64,
    pub total_forwarded: i64,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            last_msg_id: 0,
            last_msg_date: String::new(),
            last_run_at: Utc::now().to_rfc3339(),
            total_scanned: 0,
            total_forwarded: 0,
        }
    }
}

pub fn init_db(db_path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS channel_cursors (
            source_username TEXT NOT NULL,
            target_username TEXT NOT NULL,
            last_msg_id INTEGER NOT NULL DEFAULT 0,
            last_msg_date TEXT NOT NULL DEFAULT '',
            last_run_at TEXT NOT NULL DEFAULT '',
            total_scanned INTEGER NOT NULL DEFAULT 0,
            total_forwarded INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (source_username, target_username)
        )",
        [],
    )?;
    Ok(conn)
}

pub fn load_cursor(db: &Connection, source: &str, target: &str) -> anyhow::Result<Cursor> {
    let mut stmt = db.prepare(
        "SELECT last_msg_id, last_msg_date, last_run_at, total_scanned, total_forwarded
         FROM channel_cursors WHERE source_username = ?1 AND target_username = ?2",
    )?;
    let result = stmt.query_row([source, target], |row| {
        Ok(Cursor {
            last_msg_id: row.get(0)?,
            last_msg_date: row.get(1)?,
            last_run_at: row.get(2)?,
            total_scanned: row.get(3)?,
            total_forwarded: row.get(4)?,
        })
    });
    match result {
        Ok(cursor) => Ok(cursor),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Cursor::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_cursor(
    db: &Connection,
    source: &str,
    target: &str,
    cursor: &Cursor,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO channel_cursors
         (source_username, target_username, last_msg_id, last_msg_date, last_run_at, total_scanned, total_forwarded)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            source,
            target,
            cursor.last_msg_id,
            &cursor.last_msg_date,
            &cursor.last_run_at,
            cursor.total_scanned,
            cursor.total_forwarded,
        ),
    )?;
    Ok(())
}

pub fn delete_cursor(db: &Connection, source: &str, target: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM channel_cursors WHERE source_username = ?1 AND target_username = ?2",
        (source, target),
    )?;
    Ok(())
}

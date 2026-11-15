use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::sync::Mutex;

use crate::models::{Account, AttachmentMeta, Label, MessageSummary};

pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts (
    id            TEXT PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL DEFAULT '',
    picture       TEXT,
    color         TEXT NOT NULL DEFAULT '#5b8def',
    refresh_token TEXT,
    history_id    TEXT,
    last_sync     INTEGER,
    sync_error    TEXT,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    account_id   TEXT NOT NULL,
    id           TEXT NOT NULL,
    thread_id    TEXT NOT NULL DEFAULT '',
    label_ids    TEXT NOT NULL DEFAULT '[]',
    snippet      TEXT NOT NULL DEFAULT '',
    subject      TEXT NOT NULL DEFAULT '',
    from_name    TEXT NOT NULL DEFAULT '',
    from_email   TEXT NOT NULL DEFAULT '',
    to_addr      TEXT NOT NULL DEFAULT '',
    cc_addr      TEXT NOT NULL DEFAULT '',
    date         INTEGER NOT NULL DEFAULT 0,
    unread       INTEGER NOT NULL DEFAULT 0,
    starred      INTEGER NOT NULL DEFAULT 0,
    has_attach   INTEGER NOT NULL DEFAULT 0,
    body_text    TEXT,
    body_html    TEXT,
    headers      TEXT,
    PRIMARY KEY (account_id, id)
);

CREATE INDEX IF NOT EXISTS idx_messages_date ON messages(date DESC);
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(account_id, thread_id);
CREATE INDEX IF NOT EXISTS idx_messages_account ON messages(account_id, date DESC);

CREATE TABLE IF NOT EXISTS labels (
    account_id TEXT NOT NULL,
    id         TEXT NOT NULL,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL DEFAULT 'user',
    color      TEXT,
    unread     INTEGER NOT NULL DEFAULT 0,
    total      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, id)
);

CREATE TABLE IF NOT EXISTS attachments (
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    part_id    TEXT NOT NULL,
    filename   TEXT NOT NULL,
    mime_type  TEXT NOT NULL,
    size       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, message_id, part_id)
);

CREATE TABLE IF NOT EXISTS sync_state (
    account_id TEXT PRIMARY KEY,
    full_sync_done INTEGER NOT NULL DEFAULT 0
);
"#;

fn account_from_row(row: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get("id")?,
        email: row.get("email")?,
        name: row.get("name")?,
        picture: row.get("picture")?,
        color: row.get("color")?,
        last_sync: row.get("last_sync")?,
        sync_error: row.get("sync_error")?,
        unread: row.get::<_, Option<i64>>("unread")?.unwrap_or(0),
    })
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path).context("odpiranje baze")?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- settings ----------
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock();
        let v = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?;
        Ok(v)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---------- accounts ----------
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT a.*, (
                SELECT COUNT(*) FROM messages m
                 WHERE m.account_id = a.id AND m.unread = 1
                   AND m.label_ids LIKE '%\"UNREAD\"%'
                   AND m.label_ids LIKE '%\"INBOX\"%'
             ) AS unread
             FROM accounts a ORDER BY a.created_at ASC",
        )?;
        let rows = stmt.query_map([], account_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn account(&self, id: &str) -> Result<Option<Account>> {
        let conn = self.lock();
        let a = conn
            .query_row(
                "SELECT *, (
                    SELECT COUNT(*) FROM messages m
                     WHERE m.account_id = accounts.id AND m.unread = 1
                       AND m.label_ids LIKE '%\"INBOX\"%'
                 ) AS unread FROM accounts WHERE id = ?1",
                params![id],
                account_from_row,
            )
            .optional()?;
        Ok(a)
    }

    pub fn account_by_email(&self, email: &str) -> Result<Option<Account>> {
        let conn = self.lock();
        let a = conn
            .query_row(
                "SELECT *, 0 AS unread FROM accounts WHERE email = ?1",
                params![email],
                account_from_row,
            )
            .optional()?;
        Ok(a)
    }

    pub fn upsert_account(
        &self,
        id: &str,
        email: &str,
        name: &str,
        picture: Option<&str>,
        color: &str,
        refresh_token: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO accounts(id, email, name, picture, color, refresh_token, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(email) DO UPDATE SET
                name = excluded.name,
                picture = excluded.picture,
                refresh_token = COALESCE(accounts.refresh_token, excluded.refresh_token)",
            params![id, email, name, picture, color, refresh_token, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_account_profile(&self, id: &str, name: &str, picture: Option<&str>) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE accounts SET
                 name = CASE WHEN ?1 = '' THEN name ELSE ?1 END,
                 picture = ?2
             WHERE id = ?3",
            params![name, picture, id],
        )?;
        Ok(())
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM messages WHERE account_id = ?1", params![id])?;
        conn.execute("DELETE FROM labels WHERE account_id = ?1", params![id])?;
        conn.execute("DELETE FROM attachments WHERE account_id = ?1", params![id])?;
        conn.execute("DELETE FROM sync_state WHERE account_id = ?1", params![id])?;
        conn.execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn refresh_token(&self, id: &str) -> Result<Option<String>> {
        let conn = self.lock();
        let t = conn
            .query_row("SELECT refresh_token FROM accounts WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(t)
    }

    pub fn set_refresh_token(&self, id: &str, token: Option<&str>) -> Result<()> {
        let conn = self.lock();
        conn.execute("UPDATE accounts SET refresh_token = ?1 WHERE id = ?2", params![token, id])?;
        Ok(())
    }

    pub fn set_account_sync(
        &self,
        id: &str,
        history_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE accounts SET last_sync = ?1, sync_error = ?2,
                 history_id = COALESCE(?3, history_id) WHERE id = ?4",
            params![now_ms(), error, history_id, id],
        )?;
        Ok(())
    }

    pub fn history_id(&self, id: &str) -> Result<Option<String>> {
        let conn = self.lock();
        let v = conn
            .query_row("SELECT history_id FROM accounts WHERE id = ?1", params![id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(v)
    }

    // ---------- labels ----------
    pub fn replace_labels(&self, account_id: &str, labels: &[Label]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM labels WHERE account_id = ?1", params![account_id])?;
        for l in labels {
            tx.execute(
                "INSERT OR REPLACE INTO labels(account_id, id, name, kind, color, unread, total)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![account_id, l.id, l.name, l.kind, l.color, l.unread, l.total],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn labels(&self, account_id: Option<&str>) -> Result<Vec<Label>> {
        let conn = self.lock();
        let sql = "SELECT * FROM labels";
        let mut out = Vec::new();
        if let Some(aid) = account_id {
            let mut stmt = conn.prepare(&format!("{sql} WHERE account_id = ?1 ORDER BY kind, name"))?;
            let rows = stmt.query_map(params![aid], label_from_row)?;
            out.extend(rows.collect::<Result<Vec<_>, _>>()?);
        } else {
            let mut stmt = conn.prepare(&format!("{sql} ORDER BY kind, name"))?;
            let rows = stmt.query_map([], label_from_row)?;
            out.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        Ok(out)
    }

    // ---------- messages ----------
    pub fn upsert_messages(&self, account_id: &str, msgs: &[MessageSummary]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO messages(account_id, id, thread_id, label_ids, snippet, subject,
                        from_name, from_email, to_addr, date, unread, starred, has_attach)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(account_id, id) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    label_ids = excluded.label_ids,
                    snippet = excluded.snippet,
                    subject = excluded.subject,
                    from_name = excluded.from_name,
                    from_email = excluded.from_email,
                    to_addr = excluded.to_addr,
                    date = excluded.date,
                    unread = excluded.unread,
                    starred = excluded.starred,
                    has_attach = excluded.has_attach",
            )?;
            for m in msgs {
                stmt.execute(params![
                    account_id,
                    m.id,
                    m.thread_id,
                    serde_json::to_string(&m.label_ids)?,
                    m.snippet,
                    m.subject,
                    m.from_name,
                    m.from_email,
                    m.to,
                    m.date,
                    m.unread as i64,
                    m.starred as i64,
                    m.has_attachments as i64,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_labels(&self, account_id: &str, id: &str, labels: &[String]) -> Result<()> {
        let conn = self.lock();
        let unread = labels.iter().any(|l| l == "UNREAD") as i64;
        let starred = labels.iter().any(|l| l == "STARRED") as i64;
        conn.execute(
            "UPDATE messages SET label_ids = ?1, unread = ?2, starred = ?3
             WHERE account_id = ?4 AND id = ?5",
            params![serde_json::to_string(labels)?, unread, starred, account_id, id],
        )?;
        Ok(())
    }

    pub fn delete_message(&self, account_id: &str, id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM messages WHERE account_id = ?1 AND id = ?2", params![account_id, id])?;
        conn.execute(
            "DELETE FROM attachments WHERE account_id = ?1 AND message_id = ?2",
            params![account_id, id],
        )?;
        Ok(())
    }

    pub fn save_body(
        &self,
        account_id: &str,
        id: &str,
        text: Option<&str>,
        html: Option<&str>,
        headers: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE messages SET body_text = ?1, body_html = ?2, headers = ?3
             WHERE account_id = ?4 AND id = ?5",
            params![text, html, headers, account_id, id],
        )?;
        Ok(())
    }

    pub fn replace_attachments(
        &self,
        account_id: &str,
        message_id: &str,
        atts: &[AttachmentMeta],
    ) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM attachments WHERE account_id = ?1 AND message_id = ?2",
            params![account_id, message_id],
        )?;
        for a in atts {
            tx.execute(
                "INSERT OR REPLACE INTO attachments(account_id, message_id, part_id, filename, mime_type, size)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![account_id, message_id, a.part_id, a.filename, a.mime_type, a.size],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_messages(
        &self,
        account_id: Option<&str>,
        label: Option<&str>,
        query: Option<&str>,
        unread_only: bool,
        starred_only: bool,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MessageSummary>> {
        let conn = self.lock();
        let mut sql = String::from("SELECT m.* FROM messages m WHERE 1=1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(aid) = account_id {
            sql.push_str(" AND m.account_id = ?");
            args.push(Box::new(aid.to_string()));
        }
        if let Some(l) = label {
            sql.push_str(" AND EXISTS (SELECT 1 FROM json_each(m.label_ids) WHERE json_each.value = ?)");
            args.push(Box::new(l.to_string()));
        }
        if unread_only {
            sql.push_str(" AND m.unread = 1");
        }
        if starred_only {
            sql.push_str(" AND m.starred = 1");
        }
        if let Some(q) = query {
            if !q.trim().is_empty() {
                sql.push_str(
                    " AND (m.subject LIKE ? OR m.from_email LIKE ? OR m.from_name LIKE ? OR m.snippet LIKE ? OR m.to_addr LIKE ?)",
                );
                let like = format!("%{}%", q.trim());
                for _ in 0..5 {
                    args.push(Box::new(like.clone()));
                }
            }
        }
        sql.push_str(" ORDER BY m.date DESC LIMIT ? OFFSET ?");
        args.push(Box::new(limit));
        args.push(Box::new(offset));

        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), message_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn message(&self, account_id: &str, id: &str) -> Result<Option<(MessageSummary, Option<String>, Option<String>, Option<String>)>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT * FROM messages WHERE account_id = ?1 AND id = ?2",
                params![account_id, id],
                |r| {
                    let m = message_from_row(r)?;
                    Ok((
                        m,
                        r.get::<_, Option<String>>("body_text")?,
                        r.get::<_, Option<String>>("body_html")?,
                        r.get::<_, Option<String>>("headers")?,
                    ))
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn attachments(&self, account_id: &str, message_id: &str) -> Result<Vec<AttachmentMeta>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT part_id, filename, mime_type, size FROM attachments
             WHERE account_id = ?1 AND message_id = ?2",
        )?;
        let rows = stmt.query_map(params![account_id, message_id], |r| {
            Ok(AttachmentMeta {
                part_id: r.get(0)?,
                filename: r.get(1)?,
                mime_type: r.get(2)?,
                size: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_full_sync_done(&self, account_id: &str, done: bool) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO sync_state(account_id, full_sync_done) VALUES(?1, ?2)
             ON CONFLICT(account_id) DO UPDATE SET full_sync_done = excluded.full_sync_done",
            params![account_id, done as i64],
        )?;
        Ok(())
    }

    pub fn full_sync_done(&self, account_id: &str) -> Result<bool> {
        let conn = self.lock();
        let v: Option<i64> = conn
            .query_row(
                "SELECT full_sync_done FROM sync_state WHERE account_id = ?1",
                params![account_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.unwrap_or(0) == 1)
    }
}

fn label_from_row(row: &Row<'_>) -> rusqlite::Result<Label> {
    Ok(Label {
        account_id: row.get("account_id")?,
        id: row.get("id")?,
        name: row.get("name")?,
        kind: row.get("kind")?,
        color: row.get("color")?,
        unread: row.get("unread")?,
        total: row.get("total")?,
    })
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<MessageSummary> {
    let labels: String = row.get("label_ids")?;
    Ok(MessageSummary {
        account_id: row.get("account_id")?,
        id: row.get("id")?,
        thread_id: row.get("thread_id")?,
        label_ids: serde_json::from_str(&labels).unwrap_or_default(),
        snippet: row.get("snippet")?,
        subject: row.get("subject")?,
        from_name: row.get("from_name")?,
        from_email: row.get("from_email")?,
        to: row.get("to_addr")?,
        date: row.get("date")?,
        unread: row.get::<_, i64>("unread")? != 0,
        starred: row.get::<_, i64>("starred")? != 0,
        has_attachments: row.get::<_, i64>("has_attach")? != 0,
    })
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageSummary;

    fn sample(id: &str) -> MessageSummary {
        MessageSummary {
            account_id: "a1".into(),
            id: id.into(),
            thread_id: "t1".into(),
            label_ids: vec!["INBOX".into(), "UNREAD".into()],
            snippet: "povzetek".into(),
            subject: "Zadeva".into(),
            from_name: "Ana".into(),
            from_email: "ana@example.com".into(),
            to: "me@example.com".into(),
            date: 1_700_000_000_000,
            unread: true,
            starred: false,
            has_attachments: false,
        }
    }

    #[test]
    fn stores_and_filters_messages() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.upsert_account("a1", "me@example.com", "Me", None, "#fff", None)
            .unwrap();
        db.upsert_messages("a1", &[sample("m1"), sample("m2")]).unwrap();

        let inbox = db
            .list_messages(Some("a1"), Some("INBOX"), None, false, false, 50, 0)
            .unwrap();
        assert_eq!(inbox.len(), 2);

        let starred = db
            .list_messages(Some("a1"), Some("STARRED"), None, false, false, 50, 0)
            .unwrap();
        assert!(starred.is_empty());

        let accounts = db.accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].unread, 2);

        db.update_labels("a1", "m1", &["INBOX".into()]).unwrap();
        let unread = db
            .list_messages(Some("a1"), Some("INBOX"), None, true, false, 50, 0)
            .unwrap();
        assert_eq!(unread.len(), 1);
    }

    #[test]
    fn search_matches_subject_and_sender() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.upsert_account("a1", "me@example.com", "Me", None, "#fff", None)
            .unwrap();
        db.upsert_messages("a1", &[sample("m1")]).unwrap();
        let hit = db
            .list_messages(None, None, Some("Zadeva"), false, false, 50, 0)
            .unwrap();
        assert_eq!(hit.len(), 1);
        let miss = db
            .list_messages(None, None, Some("nic"), false, false, 50, 0)
            .unwrap();
        assert!(miss.is_empty());
    }
}

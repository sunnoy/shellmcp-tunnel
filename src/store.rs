use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
pub struct Store {
    db: Connection,
}
pub enum Admission {
    New,
    Saved(serde_json::Value),
    Uncertain,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        // Directory ACLs are established once; Windows icacls must not run per call.
        if !dir.join("state.db").exists() {
            crate::runtime::secure_dir(dir)?;
        }
        let db = Connection::open(dir.join("state.db"))?;
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS invocations(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL,state TEXT NOT NULL,result TEXT,created INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, invocation_id TEXT NOT NULL UNIQUE, state TEXT NOT NULL,pid INTEGER,exit_code INTEGER,created INTEGER NOT NULL); ")?;
        Ok(Self { db })
    }
    pub fn result_of(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let r: Option<Option<String>> = self
            .db
            .query_row(
                "SELECT result FROM invocations WHERE id=? AND state='completed'",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        r.flatten()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .transpose()
    }
    pub fn admit(&mut self, id: &str, fingerprint: &str) -> Result<Admission> {
        let tx = self.db.transaction()?;
        let row: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT fingerprint,result FROM invocations WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let admission = if let Some((f, result)) = row {
            ensure!(f == fingerprint, "invocation replay has changed payload");
            if let Some(r) = result {
                Admission::Saved(serde_json::from_str(&r)?)
            } else {
                Admission::Uncertain
            }
        } else {
            tx.execute(
                "INSERT INTO invocations(id,fingerprint,state,result,created) VALUES(?,?,'accepted',NULL,?)",
                rusqlite::params![id, fingerprint, crate::runtime::now()],
            )?;
            Admission::New
        };
        tx.commit()?;
        Ok(admission)
    }
    pub fn complete(&self, id: &str, result: &serde_json::Value) -> Result<()> {
        self.db.execute(
            "UPDATE invocations SET state='completed',result=? WHERE id=?",
            rusqlite::params![result.to_string(), id],
        )?;
        Ok(())
    }
    pub fn new_job(&mut self, id: &str, invocation: &str) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let n: u64 = tx.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state != 'completed'",
            [],
            |r| r.get(0),
        )?;
        ensure!(n < 32, "background job limit reached");
        tx.execute(
            "INSERT INTO jobs VALUES(?,?,'starting',NULL,NULL,?)",
            rusqlite::params![id, invocation, crate::runtime::now()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn active_job_count(&self) -> Result<u64> {
        Ok(self.db.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state != 'completed'",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn job_started(&self, id: &str, pid: u32) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET state='running',pid=? WHERE id=?",
            rusqlite::params![pid, id],
        )?;
        Ok(())
    }
    pub fn job_done(&self, id: &str, exit: i32) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET state='completed',exit_code=? WHERE id=?",
            rusqlite::params![exit, id],
        )?;
        Ok(())
    }
    pub fn job(&self, id: &str) -> Result<serde_json::Value> {
        Ok(self.db.query_row("SELECT id,state,pid,exit_code,created FROM jobs WHERE id=?",[id],|r|Ok(serde_json::json!({"job_id":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,"pid":r.get::<_,Option<u32>>(2)?,"exit_code":r.get::<_,Option<i32>>(3)?,"created_at_unix":r.get::<_,u64>(4)?})))?)
    }
    pub fn completed_jobs(&self) -> Result<Vec<(String, u64)>> {
        let mut q = self
            .db
            .prepare("SELECT id,created FROM jobs WHERE state='completed' ORDER BY created")?;
        Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn delete_job(&self, id: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM jobs WHERE id=? AND state='completed'", [id])?;
        Ok(())
    }
    pub fn list_jobs(&self) -> Result<Vec<serde_json::Value>> {
        let mut q = self
            .db
            .prepare("SELECT id FROM jobs ORDER BY created DESC LIMIT 100")?;
        let ids = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| self.job(id)).collect()
    }
}

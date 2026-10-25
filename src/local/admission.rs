use crate::{
    invocation::{Authorization, Invocation},
    store::{Admission, Store},
};
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Duration};

pub fn open(dir: &Path) -> Result<rusqlite::Connection> {
    let db = rusqlite::Connection::open(dir.join("state.db"))?;
    db.busy_timeout(Duration::from_secs(10))?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS local_job_claims(job_id TEXT PRIMARY KEY, invocation_id TEXT NOT NULL UNIQUE); CREATE TABLE IF NOT EXISTS local_job_requests(job_id TEXT PRIMARY KEY, digest TEXT NOT NULL);")?;
    Ok(db)
}

pub(crate) fn bind_job(dir: &Path, id: &str, request: &serde_json::Value) -> Result<()> {
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(request)?));
    open(dir)?.execute(
        "INSERT INTO local_job_requests VALUES(?,?)",
        rusqlite::params![id, digest],
    )?;
    Ok(())
}

pub fn fingerprint(tool: &str, arguments: &serde_json::Value) -> String {
    hex::encode(Sha256::digest(
        serde_json::json!([tool, arguments]).to_string(),
    ))
}

pub async fn call(
    dir: &Path,
    config: &super::config::LocalConfig,
    session: &str,
    request_id: &rmcp::model::RequestId,
    tool: &str,
    mut arguments: serde_json::Value,
    slots: Arc<tokio::sync::Semaphore>,
) -> Result<serde_json::Value> {
    crate::catalogue::validate(tool, &arguments)?;
    crate::policy::check(&crate::policy::load_checked(dir)?, tool)?;
    let fp = fingerprint(tool, &arguments);
    let id = hex::encode(Sha256::digest(
        serde_json::json!([config.instance_id, session, request_id]).to_string(),
    ));
    let admission = Store::open(dir)?.admit(&id, &fp)?;
    match admission {
        Admission::Saved(result) => return Ok(result),
        Admission::New => {
            let permit = match slots.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    let result = crate::invocation::result(
                        "local concurrency limit reached; submit a new request later",
                        true,
                    );
                    Store::open(dir)?.complete(&id, &result)?;
                    return Ok(result);
                }
            };
            if tool == "shell_exec" {
                arguments["timeout_s"] = serde_json::json!(
                    arguments["timeout_s"]
                        .as_u64()
                        .unwrap_or(30)
                        .min(config.max_wait_s)
                );
            }
            let invocation = Invocation {
                invocation_id: id.clone(),
                tool: tool.into(),
                arguments_json: arguments.to_string(),
                authorization: Authorization::Local {
                    instance_id: config.instance_id.clone(),
                    fingerprint: fp,
                },
            };
            let dir = dir.to_owned();
            // Detached execution persists the result even if the MCP request is cancelled.
            tokio::spawn(async move {
                let _permit = permit;
                let result = match crate::tools::execute_invocation(&dir, &invocation).await {
                    Ok(r) => r,
                    Err(e) => crate::invocation::result(e.to_string(), true),
                };
                if let Err(e) =
                    Store::open(&dir).and_then(|s| s.complete(&invocation.invocation_id, &result))
                {
                    tracing::error!(error=%e, "cannot persist local invocation result");
                }
            });
        }
        Admission::Uncertain => {}
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    loop {
        if let Some(result) = Store::open(dir)?.result_of(&id)? {
            return Ok(result);
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "invocation is running or uncertain; execution was not repeated (invocation_id={id}); inspect job_list"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn supervise(dir: &Path, id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)?;
    let config = super::config::load(dir)?;
    let path = dir.join("jobs").join(id).join("request.json");
    super::config::private_path(path.parent().unwrap(), true)?;
    super::config::private_path(&path, false)?;
    let request: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    ensure!(request["job_id"] == id, "job record identity mismatch");
    let invocation: Invocation = serde_json::from_value(request["invocation"].clone())?;
    let (instance_id, fingerprint) = match &invocation.authorization {
        Authorization::Local {
            instance_id,
            fingerprint,
        } => (instance_id, fingerprint),
    };
    ensure!(
        *instance_id == config.instance_id && invocation.tool == "shell_exec",
        "invalid local job source"
    );
    crate::policy::check(&crate::policy::load_checked(dir)?, &invocation.tool)?;
    let args: serde_json::Value = serde_json::from_str(&invocation.arguments_json)?;
    crate::catalogue::validate(&invocation.tool, &args)?;
    let mut db = open(dir)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let expected: String = tx.query_row(
        "SELECT digest FROM local_job_requests WHERE job_id=?",
        [id],
        |r| r.get(0),
    )?;
    ensure!(
        expected == hex::encode(Sha256::digest(serde_json::to_vec(&request)?)),
        "local job payload was changed"
    );
    let admitted: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs j JOIN invocations i ON i.id=j.invocation_id WHERE j.id=? AND j.invocation_id=? AND j.state='starting' AND i.fingerprint=?)",
        rusqlite::params![id, invocation.invocation_id, fingerprint], |r| r.get(0))?;
    ensure!(admitted, "local job was not admitted");
    tx.execute(
        "INSERT INTO local_job_claims VALUES(?,?)",
        rusqlite::params![id, invocation.invocation_id],
    )?;
    tx.commit()?;
    std::fs::rename(&path, path.with_file_name("consumed.json"))?;
    crate::jobs::execute_command(dir, id, &args)
}

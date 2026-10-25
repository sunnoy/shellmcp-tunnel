use serde_json::{Value, json};
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};
struct Local {
    dir: tempfile::TempDir,
    child: Child,
    url: String,
    http: reqwest::Client,
    session: String,
}
impl Drop for Local {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Local {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        shellmcp_agent::runtime::secure_dir(dir.path()).unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        drop(socket);
        shellmcp_agent::local::config::init(dir.path(), addr).unwrap();
        let child = Command::new(
            std::env::var_os("SHELLMCP_TEST_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_shellmcp-agent").into()),
        )
        .args(["local", "run", "--data-dir"])
        .arg(dir.path())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
        let mut s = Self {
            dir,
            child,
            url: format!("http://{addr}"),
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(40))
                .build()
                .unwrap(),
            session: String::new(),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = s.child.try_wait().unwrap() {
                panic!("server exited before becoming ready: {status}");
            }
            if s.http
                .get(format!("{}/readyz", s.url))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "server not ready");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let response = s.http.post(format!("{}/mcp",s.url)).header("Accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"native-test","version":"1"}}})).send().await.unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        s.session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .into();
        let _ = response.bytes().await.unwrap();
        s.http
            .post(format!("{}/mcp", s.url))
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &s.session)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        s
    }
    async fn rpc(&self, id: u32, method: &str, params: Value) -> Value {
        let response = self
            .http
            .post(format!("{}/mcp", self.url))
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let body = response.text().await.unwrap();
        if let Ok(value) = serde_json::from_str(&body) {
            return value;
        }
        body.lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|v| v["id"] == id)
            .unwrap_or_else(|| panic!("missing response: {body}"))
    }
    async fn call(&self, id: u32, name: &str, args: Value) -> Value {
        self.rpc(id, "tools/call", json!({"name":name,"arguments":args}))
            .await
    }
}
fn content(v: &Value) -> Value {
    serde_json::from_str(
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{v}")),
    )
    .unwrap()
}
#[tokio::test]
async fn mcp_shell_dedup_background_and_perimeter() {
    let s = Local::start().await;
    let list = s.rpc(1, "tools/list", json!({})).await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 15);
    let shell = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "shell_exec")
        .unwrap();
    assert_eq!(
        shell["inputSchema"]["properties"]["timeout_s"]["maximum"],
        30
    );
    let out = tempfile::tempdir().unwrap();
    let command = if cfg!(windows) {
        "Add-Content -Path count -Value x; Write-Output native"
    } else {
        "printf x >> count; printf native"
    };
    let args = json!({"command":command,"cwd":out.path(),"timeout_s":3});
    let first = s.call(2, "shell_exec", args.clone()).await;
    assert_eq!(first["result"]["isError"], false, "{first}");
    assert_eq!(content(&first)["job"]["exit_code"], 0);
    let replay = s.call(2, "shell_exec", args).await;
    assert_eq!(first, replay);
    for with_session in [false, true] {
        let mut request = s
            .http
            .post(format!("{}/mcp", s.url))
            .header("Accept", "application/json, text/event-stream");
        let mut params = json!({"name":"shell_exec","arguments":{"command":command,"cwd":out.path(),"timeout_s":3}});
        if with_session {
            request = request.header("mcp-session-id", &s.session);
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28"});
        }
        let response = request
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            400,
            "stateless delivery bypassed admission"
        );
    }
    assert_eq!(
        std::fs::read_to_string(out.path().join("count"))
            .unwrap()
            .trim(),
        "x"
    );
    assert!(
        s.call(2, "shell_exec", json!({"command":"exit 9"}))
            .await
            .get("error")
            .is_some()
    );
    assert_eq!(
        s.call(3, "shell_exec", json!({"command":"exit 23","timeout_s":3}))
            .await["result"]["isError"],
        true
    );
    let command = if cfg!(windows) {
        "Start-Sleep -Seconds 2; Write-Output done"
    } else {
        "sleep 2; printf done"
    };
    let bg = s
        .call(4, "shell_exec", json!({"command":command,"timeout_s":1}))
        .await;
    assert_eq!(content(&bg)["background"], true, "{bg}");
    let id = content(&bg)["job"]["job_id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        content(&s.call(5, "job_output", json!({"job_id":id})).await)["job"]["state"],
        "completed"
    );
    for (header, value) in [
        ("Host", "evil.example"),
        ("Origin", "https://evil.example"),
        ("Origin", "null"),
    ] {
        assert_eq!(
            s.http
                .get(format!("{}/readyz", s.url))
                .header(header, value)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    std::fs::write(s.dir.path().join("policy.json"), b"bad json").unwrap();
    assert_eq!(
        s.http
            .get(format!("{}/readyz", s.url))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert!(
        s.call(6, "shell_exec", json!({"command":"exit 0"}))
            .await
            .get("error")
            .is_some()
    );
}
#[test]
fn local_rejects_public_binding_and_legacy_data() {
    let dir = tempfile::tempdir().unwrap();
    shellmcp_agent::runtime::secure_dir(dir.path()).unwrap();
    assert!(
        shellmcp_agent::local::config::init(dir.path(), "0.0.0.0:18700".parse().unwrap()).is_err()
    );
    std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
    assert!(
        shellmcp_agent::local::config::init(dir.path(), "127.0.0.1:18700".parse().unwrap())
            .is_err()
    );
}

#[tokio::test]
async fn stopped_services_must_release_process_locks() {
    use shellmcp_agent::local::{config, diagnostics, install::Manifest};
    let dir = tempfile::tempdir().unwrap();
    shellmcp_agent::runtime::secure_dir(dir.path()).unwrap();
    let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let c = config::init(dir.path(), a.local_addr().unwrap()).unwrap();
    let health = b.local_addr().unwrap();
    drop((a, b));
    let m = Manifest {
        schema: 1,
        instance_id: c.instance_id,
        data_dir: dir.path().into(),
        release_dir: dir.path().join("releases/test"),
        version: "test".into(),
        tunnel: "unused".into(),
        tunnel_id: "unused".into(),
        health,
        services: vec![],
    };
    let lock = std::fs::File::create(dir.path().join("local.lock")).unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), diagnostics::wait_stopped(&m))
            .await
            .is_err()
    );
    drop(lock);
    // tunnel.lock need not exist when activation failed before the tunnel started.
    diagnostics::wait_stopped(&m).await.unwrap();
}

#[test]
fn local_supervisor_rejects_modified_payload_and_second_execution() {
    use sha2::{Digest, Sha256};
    use shellmcp_agent::{
        local::{admission, config},
        runtime,
        store::Store,
    };
    let dir = tempfile::tempdir().unwrap();
    runtime::secure_dir(dir.path()).unwrap();
    let c = config::init(dir.path(), "127.0.0.1:18700".parse().unwrap()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let invocation = uuid::Uuid::new_v4().to_string();
    let args = json!({"command":"exit 0"});
    let fp = admission::fingerprint("shell_exec", &args);
    let request = json!({"job_id":id,"invocation":{"invocation_id":invocation,
        "tool":"shell_exec","arguments_json":args.to_string(),
        "authorization":{"Local":{"instance_id":c.instance_id,"fingerprint":fp}}}});
    let mut store = Store::open(dir.path()).unwrap();
    store.admit(&invocation, &fp).unwrap();
    store.new_job(&id, &invocation).unwrap();
    let job = dir.path().join("jobs").join(&id);
    runtime::secure_dir(&job).unwrap();
    admission::open(dir.path())
        .unwrap()
        .execute(
            "INSERT INTO local_job_requests VALUES(?,?)",
            rusqlite::params![
                id,
                hex::encode(Sha256::digest(serde_json::to_vec(&request).unwrap()))
            ],
        )
        .unwrap();
    let mut modified = request.clone();
    modified["invocation"]["arguments_json"] = json!("{\"command\":\"exit 7\"}");
    runtime::private_write(
        &job.join("request.json"),
        &serde_json::to_vec(&modified).unwrap(),
    )
    .unwrap();
    assert!(
        admission::supervise(dir.path(), &id)
            .unwrap_err()
            .to_string()
            .contains("payload was changed")
    );
    assert_eq!(store.job(&id).unwrap()["state"], "starting");
    runtime::private_write(
        &job.join("request.json"),
        &serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    admission::supervise(dir.path(), &id).unwrap();
    assert_eq!(store.job(&id).unwrap()["exit_code"], 0);
    runtime::private_write(
        &job.join("request.json"),
        &serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    assert!(admission::supervise(dir.path(), &id).is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn scheduled_gui_host_and_shell_are_console_free() {
    if std::env::var("SHELLMCP_TEST_WINDOWS_SERVICE").as_deref() != Ok("1") {
        return;
    }
    use shellmcp_agent::local::{config, download, services};
    let dir = tempfile::tempdir().unwrap();
    shellmcp_agent::runtime::secure_dir(dir.path()).unwrap();
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    drop(socket);
    let c = config::init(dir.path(), addr).unwrap();
    let release = dir
        .path()
        .join("releases")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&release).unwrap();
    let agent = std::path::PathBuf::from(std::env::var_os("SHELLMCP_TEST_BINARY").unwrap());
    let host = download::executable(agent.parent().unwrap(), "shellmcp-host");
    std::fs::copy(host, download::executable(&release, "shellmcp-host")).unwrap();
    let defs = services::definitions(dir.path(), &release, &c.instance_id).unwrap();
    services::preflight(&defs, &[]).unwrap();
    struct Remove(shellmcp_agent::local::services::Definition);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = shellmcp_agent::local::services::remove(&self.0);
        }
    }
    let cleanup = Remove(defs[0].clone());
    services::install(&defs[0]).unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(6))
        .build()
        .unwrap();
    let url = format!("http://{addr}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = http.get(format!("{url}/readyz")).send().await
            && r.status().is_success()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "scheduled task did not start GUI host");
    let info: Value = http
        .get(format!("{url}/readyz"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = info["pid"].as_u64().unwrap();
    let script = format!(
        "$ErrorActionPreference='Stop'; $p=Get-Process -Id {pid}; if($p.SessionId -eq 0){{throw 'not in interactive session'}}; if($p.MainWindowHandle -ne 0){{throw 'unexpected GUI window'}}; $c=Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}'; if($c.Name -contains 'conhost.exe'){{throw 'console host allocated'}}; Write-Output ('session='+$p.SessionId)"
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .unwrap();
    assert!(status.success());
    // Re-registering the same task is idempotent; exactly the same task name is reused.
    services::stop(&defs[0]).unwrap();
    services::install(&defs[0]).unwrap();
    let mut restarted = false;
    for _ in 0..100 {
        if let Ok(response) = http.get(format!("{url}/readyz")).send().await
            && response.status().is_success()
        {
            restarted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        restarted,
        "restarted task failed: {}",
        std::fs::read_to_string(dir.path().join("host.log")).unwrap_or_default()
    );
    services::remove(&defs[0]).unwrap();
    std::mem::forget(cleanup);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

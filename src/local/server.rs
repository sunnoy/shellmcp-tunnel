use super::config::LocalConfig;
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::*,
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
struct App {
    dir: PathBuf,
    config: LocalConfig,
    release_dir: Option<PathBuf>,
    business: Arc<tokio::sync::Semaphore>,
    management: Arc<tokio::sync::Semaphore>,
}
struct Handler {
    app: App,
    session: String,
}
fn error(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("shellmcp-tunnel", env!("CARGO_PKG_VERSION")))
            .with_instructions("Native local tools. Shell waiting is capped at 30 seconds; timed-out jobs continue. Use job_output/job_list; do not blindly retry side effects.")
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25))
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let policy = crate::policy::load_checked(&self.app.dir).map_err(error)?;
        let mut tools = Vec::new();
        for definition in crate::catalogue::catalogue()["tools"].as_array().unwrap() {
            let mut value = definition.clone();
            let name = value["name"].as_str().unwrap();
            if crate::policy::check(&policy, name).is_err() {
                continue;
            }
            if name == "shell_exec" {
                value["inputSchema"]["properties"]["timeout_s"]["maximum"] =
                    self.app.config.max_wait_s.into();
                value["description"] = serde_json::json!(format!(
                    "{} Local wait limit: {}s; longer commands continue as background jobs.",
                    value["description"].as_str().unwrap_or_default(),
                    self.app.config.max_wait_s
                ));
            }
            tools.push(serde_json::from_value(value).map_err(error)?);
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let slots = if matches!(
            request.name.as_ref(),
            "get_environment" | "job_list" | "job_output" | "job_kill"
        ) {
            self.app.management.clone()
        } else {
            self.app.business.clone()
        };
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let result = super::admission::call(
            &self.app.dir,
            &self.app.config,
            &self.session,
            &context.id,
            &request.name,
            arguments,
            slots,
        )
        .await
        .map_err(error)?;
        serde_json::from_value::<CallToolResult>(result)
            .map(Into::into)
            .map_err(error)
    }
}

async fn perimeter(State(app): State<App>, mut request: Request, next: Next) -> Response {
    if request
        .headers()
        .get("mcp-protocol-version")
        .is_some_and(|v| {
            !matches!(
                v.to_str(),
                Ok("2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25")
            )
        })
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"unsupported_protocol_version"})),
        )
            .into_response();
    }
    let authority = app.config.listen.to_string();
    let localhost = format!("localhost:{}", app.config.listen.port());
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !host.is_some_and(|h| h == authority || h == localhost)
        || request.headers().contains_key(header::ORIGIN)
            && origin.is_none_or(|o| o != format!("http://{}", host.unwrap_or_default()))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"host_or_origin_denied"})),
        )
            .into_response();
    }
    if request.method() == axum::http::Method::POST && request.uri().path() == "/mcp" {
        let (parts, body) = request.into_parts();
        let bytes = match axum::body::to_bytes(body, 1_100_000).await {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        // Pin the stateful transport: SDK stateless metadata must not create a
        // fresh admission namespace for every delivery of the same request.
        if value["params"]["_meta"]
            .get("io.modelcontextprotocol/protocolVersion")
            .is_some()
            || value["method"] != "initialize" && !parts.headers.contains_key("mcp-session-id")
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":"stateful_session_required"})),
            )
                .into_response();
        }
        request = Request::from_parts(parts, axum::body::Body::from(bytes));
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

pub async fn readiness(dir: &Path) -> Result<serde_json::Value> {
    let c = super::config::load(dir)?;
    ensure!(
        crate::policy::load_checked(dir)?.mode != crate::policy::Mode::Disabled,
        "local policy is disabled"
    );
    crate::store::Store::open(dir)?;
    for parent in [dir.to_path_buf(), dir.join("jobs")] {
        let probe = parent.join(format!(".ready-{}", uuid::Uuid::new_v4()));
        crate::runtime::private_write(&probe, b"ready")?;
        std::fs::remove_file(probe)?;
    }
    let mut cmd = tokio::process::Command::new(if cfg!(windows) {
        "powershell.exe"
    } else {
        crate::environment::exec_shell()
    });
    if cfg!(windows) {
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"]);
    } else {
        cmd.args(["-c", "exit 0"]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    ensure!(
        tokio::time::timeout(Duration::from_secs(5), cmd.status())
            .await??
            .success(),
        "shell readiness failed"
    );
    Ok(
        serde_json::json!({"ready":true,"mode":"local","version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"instance_id":c.instance_id,"listen":c.listen,"max_wait_s":c.max_wait_s}),
    )
}

async fn ready(State(app): State<App>) -> Response {
    match readiness(&app.dir).await {
        Ok(mut value) => {
            value["release_dir"] = serde_json::json!(app.release_dir);
            Json(value).into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"ready":false,"error":e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn run(dir: &Path, release_dir: Option<&Path>) -> Result<()> {
    let config = super::config::load(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("local.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|e| anyhow::anyhow!("local agent lock: {e}"))?;
    readiness(dir).await?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let app = App {
        dir: dir.to_owned(),
        config,
        release_dir: release_dir.map(Path::to_owned),
        business: Arc::new(tokio::sync::Semaphore::new(8)),
        management: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    let cancellation = tokio_util::sync::CancellationToken::new();
    let mut settings = StreamableHttpServerConfig::default();
    settings.allowed_hosts = vec![
        app.config.listen.to_string(),
        format!("localhost:{}", app.config.listen.port()),
    ];
    settings.max_request_body_bytes = 1_100_000;
    settings.cancellation_token = cancellation.child_token();
    let factory_app = app.clone();
    let service = StreamableHttpService::new(
        move || {
            Ok(Handler {
                app: factory_app.clone(),
                session: uuid::Uuid::new_v4().to_string(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        settings,
    );
    let router = Router::new().route_service("/mcp", service)
        .route("/healthz", get(|| async { Json(serde_json::json!({"ok":true,"mode":"local","version":env!("CARGO_PKG_VERSION")})) }))
        .route("/readyz",get(ready))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(middleware::from_fn_with_state(app.clone(),perimeter)).with_state(app);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let stop = async {
                #[cfg(unix)]
                {
                    if let Ok(mut signal) =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    {
                        signal.recv().await;
                        return;
                    }
                }
                std::future::pending::<()>().await;
            };
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = stop => {} }
            cancellation.cancel();
        })
        .await?;
    Ok(())
}

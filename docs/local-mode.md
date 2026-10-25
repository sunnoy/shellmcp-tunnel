# Rust 本地模式操作

## 构建与分发

`cargo build --release --locked --bin shellmcp-agent --bin shellmcp-host` 构建本地 MCP 和隧道管理程序，不包含云注册、签名授权或 WebSocket 适配器。无需 Python。Windows 的两个 exe 必须放在同一发行目录：CLI 用于安装/诊断，GUI 子系统 host 用于计划任务。

将各原生平台的产物交给打包器，例如：

```sh
cargo run --bin package-local -- \
  --out /private/local-release \
  --base-url https://YOUR-RELEASE-HOST/local/0.3.0 \
  --artifact linux-amd64=target/release/shellmcp-agent \
  --artifact windows-amd64=/private/windows/shellmcp-agent.exe \
  --host-artifact windows-amd64=/private/windows/shellmcp-host.exe
```

支持平台名：`linux-amd64`、`linux-arm64`、`darwin-amd64`、`darwin-arm64`、`windows-amd64`、`windows-arm64`。只为实际提供的产物生成下载映射；未提供的平台直接报错。发布目录需要由维护者另行上传到受信 HTTPS 站点，本工具不会上传。

引导脚本内嵌原生产物的 SHA-256，执行前检查非空和摘要。信任根是用户取得的可信引导脚本；本地模式尚无独立签名的更新清单/自动更新链路。官方 tunnel-client 固定为经测试的 v0.0.16，摘要直接内置在 Rust 安装器，更新版本须重新审核并构建。下载包中的许可证保留。

生成的 Windows 引导脚本参数：

```powershell
.\install-local.ps1 -TunnelId tunnel_你的ID -KeyFile "$env:USERPROFILE\tunnel-runtime.key"
```

Unix 引导脚本参数直接转交 Rust CLI：

```sh
bash install-local.sh --tunnel-id tunnel_你的ID --key-file "$HOME/tunnel-runtime.key"
```

不要直接执行仓库里的模板；只有打包后的脚本包含有效的产物 URL 和摘要。当前未发布公共安装 URL。

## 状态和服务

- MCP：`http://127.0.0.1:18700/mcp`，`/healthz` 表示存活，`/readyz` 实测策略可用、数据库、写入和真实 shell。
- 隧道：`http://127.0.0.1:18710/readyz`；`local doctor` 还检查成功的 OpenAI 轮询记录、MCP 启动探测和发行身份。
- 数据、策略、审计、jobs、install.json 在本地模式专属目录。每个发行目录保留独立 tunnel profile 和私有 runtime key 引用。
- 服务名含实例 ID 前缀：Linux `shellmcp-local-<实例前12位>-agent/tunnel.service`；macOS `com.shellmcp-local-…` LaunchAgents；Windows 同名登录计划任务。
- 服务以安装用户身份运行。Windows/macOS 需要该用户登录；Linux 无 lingering 时随用户登录会话退出。安装器不提权或切换为 SYSTEM。
- Windows `host.log` 记录宿主错误；`tunnel.log` 记录官方隧道日志。Linux MCP 日志在 user journal，macOS 使用本安装的日志文件。

CLI 不会添加到全局 PATH。可从下载包运行，也可以从 `install.json` 的 `release_dir` 找到已经安装的 `shellmcp-agent`。非默认数据目录须在所有命令后加 `--data-dir`。

## 更新、故障和卸载

更新阶段先准备新发行目录，校验官方下载包，再写事务记录；停止旧服务、先启动并验证 MCP、再启动并验证隧道。两者成功才提交 `install.json`。失败保留原身份和数据库，恢复旧服务。被强杀的安装用 `local recover` 按事务记录恢复。

安装成功只表示本地两个组件已就绪；ChatGPT 工作区授权和连接器实际调用是独立验收项目。API 账户余额不影响 tunnel-client 轮询，但会影响用 Responses API 发起模型测试。

卸载拒绝仍有未完成作业的实例，不删除 OpenAI 远端 tunnel。默认保留数据、key 和旧发行文件，方便恢复和诊断；`--purge` 才删除目录。Windows 不能删除正在运行的自身 exe，须从安装目录外的下载包运行 purge。

后台作业在 MCP 服务重启后继续运行。安装更新会关闭旧 MCP 会话；重连后的新会话不能继承旧请求 ID 的去重命名空间，应先查询作业列表。完成作业有 7 天、500 个、256 MiB 日志清理阈值；本地 invocation 去重结果目前持续保留，尚无自动清理策略。

## 验证范围

本次在 Linux 运行 Rust 单元/集成测试，包含子进程收割、HTTP MCP 初始化、工具目录、实际 shell、非零退出码、后台超时、请求去重冲突、错误 Host/Origin 和损坏策略。另以实际 systemd 用户服务验证无效隧道凭据的失败回滚，两个服务、监听和事务记录均清理。

Windows 使用 x86_64 实机运行本地 MCP 回归，另通过真正的交互登录计划任务验证 GUI 宿主；测试不是单纯交叉编译。发现并修复了任务 XML 编码、当前用户 SID、Windows 路径规范化以及回滚进程锁/启动顺序问题。完整结果和未完成项见 [2026-10-25 验收记录](local-acceptance.md)。

macOS 的原生编译和工具测试由 CI 验证，LaunchAgent 安装/回滚尚未实机验收。Windows ARM64 和 Linux ARM64 未做本次运行验证。不得把跨平台编译或工具测试描述为所有平台完整安装已实测。

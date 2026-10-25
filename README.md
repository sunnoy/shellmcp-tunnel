# ShellMCP Tunnel

原生 Rust 本地 MCP 服务，通过官方 OpenAI `tunnel-client` 接入 ChatGPT。Linux、macOS、Windows 共用执行核心，安装器下载并管理官方隧道客户端，安装和运行无需 Python。

本仓库专用于本地 MCP + OpenAI 隧道，已经移除原 ShellMCP 云平台注册、WebSocket、签名许可及云端更新实现。项目名为 `shellmcp-tunnel`，二进制名 `shellmcp-agent`、`shellmcp-host` 和 `local` 子命令沿用原生安装接口。

当前为早期版本：Linux x86_64 与 Windows x86_64 已实测，macOS 服务安装和 ARM64 完整安装尚未实机验收，ChatGPT 外部工具调用也仍待验收。具体见 [验收记录](docs/local-acceptance.md)。使用前阅读 [安全边界](SECURITY.md)。

## 安装本地模式

先从源码构建 `shellmcp-agent`（见下文）；Windows 还需同目录的 `shellmcp-host.exe`。然后运行一次：

```powershell
.\shellmcp-agent.exe local install --tunnel-id tunnel_你的ID --key-file "$env:USERPROFILE\tunnel-runtime.key"
```

```sh
./shellmcp-agent local install --tunnel-id tunnel_你的ID --key-file "$HOME/.config/openai/tunnel-runtime.key"
```

这条命令安装 Rust agent、下载并校验官方 tunnel-client、生成私有配置、注册两个用户后台服务，并等待本地 MCP 就绪和 OpenAI 轮询成功。Windows 计划任务直接运行 GUI 子系统的 Rust 宿主，业务 PowerShell 和 tunnel-client 都以无控制台方式创建。

必填参数只有 Tunnel ID 和 key 来源（首次安装）。key 文件只包含密钥本身；不要把密钥作为命令参数。也可以使用 `--key-env OPENAI_TUNNEL_KEY` 指定环境变量**名称**，安装器会导入到私有文件。重装可复用已经保存的 key。

| 参数 | 默认值 / 用途 |
|---|---|
| `--tunnel-id` | OpenAI Tunnel ID；独立机器使用独立隧道 |
| `--key-file` / `--key-env` | 二选一；runtime key 需要 Tunnels Read + Use |
| `--data-dir` | Windows `%LOCALAPPDATA%\ShellMCP-Agent-Local`；Unix `~/.local/share/shellmcp-agent-local` |
| `--listen` | `127.0.0.1:18700`，MCP 路径 `/mcp` |
| `--health` | `127.0.0.1:18710`，官方隧道健康面 |
| `--tunnel-archive` | 可选离线缓存，必须匹配内置的官方版本和 SHA-256 |
| `--dry-run` | 只显示脱敏参数，不安装 |

当前只发布源码，尚未提供二进制发行包或一键下载 URL。可从源码构建；`package-local` 可生成下载两个原生组件的 `install-local.sh` / `install-local.ps1` 引导脚本，但生成不等于发布。具体见 [本地模式操作与验收](docs/local-mode.md)。

## 使用

ChatGPT 新建自定义 MCP：Connection 选择 **Tunnel**，选择安装时的 Tunnel ID，认证选择 **No Auth**。需要让隧道关联目标 ChatGPT 工作区；安装器不会创建隧道或修改工作区权限。[OpenAI 官方说明](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)

```sh
shellmcp-agent local doctor                # 同时检查实际运行的两个组件
shellmcp-agent local policy --mode read_only  # full | read_only | disabled
shellmcp-agent local audit --tail 20
shellmcp-agent local recover               # 仅用于中断的安装事务
shellmcp-agent local uninstall             # 移除服务，保留本地数据和发行文件
shellmcp-agent local uninstall --purge     # 额外删除本安装目录；拒绝仍有后台作业
```

独立测试 MCP，无需隧道或任何 key：

```sh
shellmcp-agent local init
shellmcp-agent local run
```

15 个工具来自 [统一工具目录](protocol/tools.json)：文件列表、读取、搜索、写入、编辑、移动、建目录，环境信息，以及 shell 和后台作业管理。Unix 用 bash/sh，Windows 用 PowerShell。`timeout_s` 最大同步等待 30 秒，超时返回 `job_id`，命令继续运行；用 `job_output` 查询。非零退出码标为工具错误。当前没有 PTY 或交互 stdin。

Host/Origin 校验、仅回环监听、无 CORS。副作用按实例、会话、JSON-RPC 请求 ID 去重；相同 ID 修改参数会拒绝。进程崩溃后不自动重放执行状态不明确的请求。重连建立新会话属于新请求，调用者须先查询已有 job。这是当前 OS 用户权限下的 shell，不是容器沙箱。

## 开发

```sh
git clone https://github.com/sunnoy/shellmcp-tunnel.git
cd shellmcp-tunnel
cargo build --release --locked --bin shellmcp-agent --bin shellmcp-host
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

使用支持 Rust 2024 edition 的稳定工具链和平台原生 C/C++ 编译工具。构建产物在 `target/release/`；运行时不需要 Rust 工具链。当前没有定时自动更新；更新时用新构建或新发行包重新执行安装命令，安装器替换两个服务并验证，失败则恢复之前的服务定义和发行目录。官方 tunnel-client 版本固定为 v0.0.16，不跟踪 latest。

打包、服务管理和平台限制见 [操作文档](docs/local-mode.md)，验证范围见 [验收记录](docs/local-acceptance.md)。

## 许可证

本项目使用 [Apache-2.0](LICENSE)。官方 tunnel-client 是独立项目，安装器保留其发行包中的许可证文件；本项目不是 OpenAI 官方产品。

# 验收记录：2026-10-25

代码版本：Rust agent 0.3.0；官方 tunnel-client 固定 v0.0.16。本记录包含安装流程测试和源码验证。服务安装测试环境为 Linux x86_64 与 Windows x86_64；其余平台的服务生命周期尚未实机验收。

## 安装流程测试

以下安装流程使用早期测试构建验证，早于下述 TLS/ZIP 依赖升级，不代表最新构建已重复完成全部服务生命周期验收。

| 项目 | 结果与判据 |
|---|---|
| 僵尸收割 | 6 项真实子进程回归通过，覆盖完成、超时、调用取消、数据库/输出错误和异常退出。 |
| Linux Rust 回归 | MCP 与执行核心测试通过：6 项收割、4 项本地 MCP/安全/服务锁，及安全和文件工具回归 |
| Windows Rust 回归 | Windows 原生运行 5 项通过，含真实交互登录计划任务，并验证文件工具。不是仅交叉编译 |
| MCP 执行 | 初始化、15 个工具、真实 PowerShell/bash、非零退出码、后台作业、相同请求重投去重、同 ID 改参拒绝、Host/Origin 拦截、策略损坏拒绝 |
| 本地请求安全 | 无会话调用和 SDK 无状态元数据不能绕过去重；修改 job 请求内容和第二次执行同一 job 均被拒绝 |
| 安装与重跑 | Windows 双计划任务安装、正常重装通过；doctor 核对实际 agent 版本、实例、发行目录及 OpenAI 成功轮询 |
| 失败回滚 | Windows 错误 key 导致安装非零退出，旧发行恢复健康，install.json 不变，transaction.json 清理；45 秒真实后台作业跨越更新并完成 |
| Linux 实际服务回滚 | release 测试构建配无效凭据进行首次安装，失败后两个 systemd user unit 文件、监听及事务/安装清单均清理，MainPID 均为 0 |
| 崩溃恢复 | 首轮回滚故障留有事务记录，修复后的 `local recover` 成功恢复旧服务并清理记录 |
| 包篡改拦截 | Windows 使用损坏的 tunnel archive，SHA-256 校验失败；原服务 PID 和已提交发行未改变 |
| 端口冲突拦截 | Windows 人为占用目标端口，新安装被拒绝，不创建计划任务；已有实例未受影响 |
| 作业卸载保护 | Windows 运行真实后台作业时拒绝卸载，任务、监听、原 agent PID 保持不变 |
| 默认卸载 | 作业完成后移除两个计划任务，18700/18710 无监听、install.json 移除；state.db 保留 |
| 卸载后重装 | Windows 不传离线缓存，从官方 GitHub 地址重新下载并校验 tunnel-client，安装成功且 doctor 两组件健康 |
| 原生引导脚本 | 早期打包后的 PowerShell 脚本在 Windows 运行通过摘要验证和原生 CLI dry-run；篡改 exe 被拒绝，临时目录清理。此项模拟了产物下载传输，不作为公共 Release 下载的验收证据 |
| 静态检查 | fmt、clippy（warnings 为错误）、shellcheck、diff whitespace 与文档本地链接检查通过 |

Windows 默认目录为 `%LOCALAPPDATA%\ShellMCP-Agent-Local`，运行两个当前用户登录任务：`shellmcp-local-<实例前12位>-agent` 和 `shellmcp-local-<实例前12位>-tunnel`。MCP 为 `127.0.0.1:18700/mcp`，隧道健康面为 `127.0.0.1:18710`。runtime key 存储在限制 ACL 的文件内，不作为命令行参数或仓库内容。验收时核对了运行文件的 SHA-256 与构建产物一致；日常状态以 `local doctor` 为准。

## Windows 实测修复

- 计划任务 XML 改为带 BOM 的 UTF-16；当前用户使用实际 SID，避免 SSH 环境下 `USERDOMAIN=WORKGROUP` 导致注册失败。
- 安装清单读取统一规范化路径，避免 `\\?\` 前缀造成身份比较失败。
- 停止计划任务后，回滚和更新都等待端口消失、两个进程锁释放；仅任务状态变为 Ready 不足以证明旧进程已经退出。
- 恢复旧版本时也先验证 MCP 就绪，再启动 tunnel-client，避免隧道启动探针过早失败。
- 计划任务直接执行 Rust GUI 子系统宿主；桌面窗口枚举没有新增 ShellMCP 窗口，宿主和业务 PowerShell 无可见窗口。官方 tunnel-client 会保留一个无可见窗口的 `conhost.exe`，不应把“无弹窗”误写为整个进程树不存在 conhost。

## 尚未完成或不包含

- **ChatGPT 外部工具调用未通过验收。** 官方隧道已成功初始化 MCP 并轮询 OpenAI；尚无 ChatGPT 工具调用成功的验收记录。
- macOS 的编译和工具测试已通过 CI，服务安装与回滚尚未实机验收；Linux ARM64、Windows ARM64 未做本次实机运行验证。
- 目前无定时自动更新、无独立签名更新清单。新发行包重跑安装器可替换两组件；tunnel-client 版本和 SHA-256 随 agent 源码固定，不追踪 latest。
- 早期 `target/local-distribution-20261025` 是使用占位 URL 的本地产物，不作为公共发行包。公开发行由 [发布工作流](../.github/workflows/release.yml) 从对应标签重新构建和打包。
- `local uninstall --purge` 已实现，但本轮 Windows 操作测试使用默认保留数据的卸载；未删除真实测试实例的数据目录。卸载不会删除 OpenAI 远端 tunnel。
- 本地 invocation 去重结果尚未 GC；默认卸载保留 key、作业数据及旧发行目录。

测试机已有的其他 MCP 实例保持原 PID 和启动时间，安装测试使用独立目录与端口。

参数和日常操作见 [使用与维护](usage.md)。使用说明以 README 和操作文档为准。

## 源码与构建检查

TLS/ZIP 依赖使用 `rustls-webpki 0.103.16`、`webpki-roots 1.0.11`、`zip 9.1.0`。旧测试安装包早于这些依赖升级，不作为本仓库的发行产物。

源码验证：

- Linux：19 项默认测试通过，另显式运行官方 ZIP 解压/执行测试通过（共 20 项）。
- Windows x86_64 实机：15 项通过，包含 MCP、文件工具、真实无窗口计划任务、官方 ZIP 解压/执行及损坏包拒绝。
- Linux 和 Windows GNU 目标的 clippy（warnings 为错误）通过；fmt、shellcheck、本地文档链接检查通过。
- cargo-audit 0.22.2 / RustSec 2026-10-24：0 个已知漏洞，无忽略项。Gitleaks 8.30.1 扫描源码未发现密钥。
- 三平台 CI：[跨平台构建与测试](https://github.com/sunnoy/shellmcp-tunnel/actions/runs/40184182785)。CI 不包含真实桌面登录任务或外部 ChatGPT 调用；真实 Windows 桌面测试通过 `SHELLMCP_TEST_WINDOWS_SERVICE=1` 显式开启。

官方压缩包测试默认忽略，需提供对应平台 v0.0.16 包后显式执行：

```sh
SHELLMCP_TEST_TUNNEL_ARCHIVE=/path/to/official.zip cargo test --locked --test download -- --include-ignored
```

Windows 测试机已通过 `local install` 升级为本次测试构建：实例 ID 与数据库保留，发行目录替换，安装后的 agent/host SHA-256 与本次构建一致。doctor 验证运行中的 MCP 发行身份、真实 shell 和成功的 OpenAI 轮询，安装事务记录已清理。这不替代外部 ChatGPT 工具调用验收。

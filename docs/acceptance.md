# 验收记录：2026-10-25

早期安装测试版本：Rust agent 0.3.0；当前公开发行版本：v0.3.2。官方 tunnel-client 固定 v0.0.16。本记录包含安装流程测试和源码验证。服务安装测试环境为 Linux x86_64 与 Windows x86_64；其余平台的服务生命周期尚未实机验收。

## 二进制发行与下载验证

[v0.3.2 Release](https://github.com/sunnoy/shellmcp-tunnel/releases/tag/v0.3.2) 已公开：Linux、macOS 的 x86_64 / ARM64，以及 Windows x86_64，共五个平台。产物取自 [v0.3.2 构建记录](https://github.com/sunnoy/shellmcp-tunnel/actions/runs/40190351287) 中通过原生测试和构建的五个任务，已核对对应源码提交。该轮 Windows ARM64 任务失败，整体自动发布任务跳过；此次仅组装并上传五个成功任务的产物。后续发布工作流已明确采用这五个平台。

- 11 个公开附件包含原生程序、安装脚本、产物映射、许可证和校验清单；GitHub 返回的每个附件 SHA-256 均与本地产物一致。
- Linux x86_64 和 Windows x86_64 测试机直接执行操作文档中的下载命令，仅替换占位参数并启用 dry-run。脚本从公开 `releases/latest/download` 地址取得，实际原生程序下载、SHA-256 校验和 CLI 启动均通过，无需 clone 或编译。
- Windows 验证后，计划任务、相关进程、18700/18710 监听和新增引导脚本临时文件均为 0；两台机器均未创建 dry-run 指定的安装数据目录。
- Windows ARM64 曾出现首次 PowerShell 探测超过 5 秒，以及任务在同步等待期内未完成的测试失败。启动探测已放宽至 15 秒，日常健康探测仍为 5 秒；该架构暂列实验性 CI，不提供发行包。

这轮验证覆盖公开下载和原生启动，不代表 v0.3.2 已重新完成全部服务生命周期或外部 ChatGPT 工具调用验收。Windows x86_64 程序依赖微软 Visual C++ v14 运行库，准备方式见[安装说明](usage.md#安装)。

## README 两参数命令验证

测试 [README 安装命令](../README.md#安装)（提交 `dbf95af`），从公开 `releases/latest/download` 下载 v0.3.2，在 Linux x86_64 和 Windows x86_64 / PowerShell 5.1 实机执行。用测试 ID 和无效测试 key 替换末尾两个参数；下载测试追加 dry-run，服务测试仅追加独立数据目录与 `28700/28710` 端口，下载地址、校验和参数转换保持原样。

| 项目 | 实际结果 |
|---|---|
| 公开 Release 下载 | 两平台安装脚本和原生二进制下载、SHA-256 校验、原生 CLI dry-run 通过 |
| 两参数传递 | 第一个参数成为 Tunnel ID；第二个参数经环境变量导入，核对生成的 runtime.key 与输入完全一致，无需用户提供文件路径 |
| 密钥处理 | Linux 文件权限仅当前用户可读写，Windows ACL 仅授予当前用户；未在测试输出中回显 key，Windows 命令恢复原环境变量 |
| 组件启动 | 从 OpenAI 官方 Release 下载并校验 tunnel-client；两平台 MCP `/readyz` 和隧道健康接口均返回 200 |
| 无效 key 回滚 | OpenAI 返回 401，安装命令非零退出并自动回滚；新建服务/计划任务、进程、监听及未完成事务均无残留 |
| 命令故障处理 | 下载失败、空下载、安装器非零退出均正确判失败并清理临时脚本；Windows 在 PowerShell 5.1 验证了环境变量恢复 |
| 原环境保护 | Linux 原有服务 PID、启动时间不变；Windows 默认安装数据未改变，测试目录已清理，仍无 ShellMCP 任务、进程或监听 |

另验证了格式错误的 Tunnel ID 会被官方客户端拒绝；其格式为 `tunnel_` 后接 32 位小写字母或数字。测试 key 的 401 是预期失败，不代表真实隧道已连接或 ChatGPT 外部调用已验收。本轮未在 macOS 或 WorkBuddy 沙箱中执行。

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
- macOS 和 Linux ARM64 的编译与工具测试已通过原生 CI，完整服务安装与回滚尚未实机验收；Windows ARM64 尚未通过稳定性验收。
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

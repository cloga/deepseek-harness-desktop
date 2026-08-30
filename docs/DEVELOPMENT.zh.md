# 开发

DeepSeek Harness Desktop 是 **Tauri 2 + React 18** 应用：前端位于 `src/`，Rust 后端位于 `src-tauri/`。

## 环境要求

| 工具 | 版本 |
| --- | --- |
| Node.js | 20+ |
| Rust | 1.77.2+ |
| pnpm | 9+ |

以及平台编译工具链：

- **Windows** — MSVC 构建工具 + WebView2
- **macOS** — Xcode Command Line Tools
- **Linux** — WebKit2GTK

## 常用命令

```bash
pnpm install      # 安装依赖
pnpm dev          # 前端开发服务器（Vite）
pnpm typecheck    # 前端 TypeScript 检查
pnpm tauri dev    # 调试模式运行桌面端
pnpm tauri build  # 构建安装包
```

后端检查（在 `src-tauri/` 下执行）：

```bash
cargo check
cargo test
```

macOS 的 Developer ID 签名、公证与 GitHub Actions Secrets 配置见 [macOS 签名与公证](./MACOS_SIGNING.zh.md)。

若要新增一个随安装包分发、内置在应用里的插件，请参阅 [内置插件（Internal Plugins）](./BUILTIN_PLUGINS.zh.md)。

## 小贴士

- 调试模式使用 **3081** 端口，正式版使用 **3080** —— 两者互不冲突，可以同时运行已安装版本与开发构建。

## 使用本地 dsh 构建

桌面端的**本地**核心是指通过 npm/pnpm 全局安装的 `@deepseek-ai/dsh`，不是预打包列表中的最新一行。以 `src-` 开头的预打包版本，是 `deepseek-harness-pkg` 在对应版本尚未发布到 npm 时，根据上游 GitHub release tag 远端构建的预览发行版；它与本机 checkout 没有关系。

推荐使用显式、可回滚的开发配置：

```powershell
cd C:\path\to\deepseek-harness\apps\cli
npm link --ignore-scripts --no-audit --no-fund
[Environment]::SetEnvironmentVariable(
  'DSH_CLI_PATH',
  (Join-Path (npm prefix -g) 'dsh.cmd'),
  'User'
)
```

修改全局 link 或环境变量后，需要完整退出并重新打开桌面端。选择**本地**行，并确认界面展示的“入口”已解析到 checkout。每次创建新的 Harness 进程时，桌面端日志会记录选中的 `source` 与准确 `entry`。已有会话会缓存工具 schema；验证核心改动时必须新建会话。

若要移除显式覆盖，执行 `[Environment]::SetEnvironmentVariable('DSH_CLI_PATH', $null, 'User')`。该配置不会修改 `$DSH_HOME`、档案、会话或凭据。

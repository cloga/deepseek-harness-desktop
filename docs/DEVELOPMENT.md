# Development

DeepSeek Harness Desktop is a **Tauri 2 + React 18** app: the UI lives in `src/`, the Rust backend in `src-tauri/`.

## Requirements

| Tool | Version |
| --- | --- |
| Node.js | 20+ |
| Rust | 1.77.2+ |
| pnpm | 9+ |

Plus the platform toolchain:

- **Windows** — MSVC build tools + WebView2
- **macOS** — Xcode Command Line Tools
- **Linux** — WebKit2GTK

## Commands

```bash
pnpm install      # install dependencies
pnpm dev          # frontend dev server (Vite)
pnpm typecheck    # frontend TypeScript check
pnpm tauri dev    # run the desktop app in debug mode
pnpm tauri build  # build installers
```

Backend checks (from `src-tauri/`):

```bash
cargo check
cargo test
```

For Developer ID signing, notarization, and the required GitHub Actions secrets, see [macOS signing and notarization](./MACOS_SIGNING.md).

To add a new built-in (internal) plugin bundled with the app, see [Built-in (Internal) Plugins](./BUILTIN_PLUGINS.md).

## Tips

- Debug mode serves on port **3081**, release builds on **3080** — the two never clash, so you can run an installed copy and a dev build side by side.

## Using a local dsh build

Desktop's **Local** core means an npm/pnpm globally installed `@deepseek-ai/dsh`; it does not mean the newest row in the bundled release list. A bundled version prefixed with `src-` is built by `deepseek-harness-pkg` from an upstream GitHub release tag because that version was not yet published to npm. It has no relationship to a checkout on this machine.

For an explicit, reversible development setup:

```powershell
cd C:\path\to\deepseek-harness\apps\cli
npm link --ignore-scripts --no-audit --no-fund
[Environment]::SetEnvironmentVariable(
  'DSH_CLI_PATH',
  (Join-Path (npm prefix -g) 'dsh.cmd'),
  'User'
)
```

Fully exit and reopen Desktop after changing the global link or environment. Select the **Local** row and verify its displayed Entry resolves into the checkout. Desktop logs the selected `source` and exact `entry` whenever it starts a new Harness process. Existing sessions cache their tool schemas; create a new session when validating a core change.

To remove the explicit override, run `[Environment]::SetEnvironmentVariable('DSH_CLI_PATH', $null, 'User')`. This does not alter `$DSH_HOME`, profiles, sessions, or credentials.

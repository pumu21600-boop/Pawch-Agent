# Pawch Agent

Windows 上的本地优先自主办事 Agent。Rust（axum）+ Svelte，SSE 流式。

## 功能

- **自主循环**：模型带工具决策 → 本地执行 → 结果回填，直到完成；同轮只读工具并行，写冲突自动跳过。
- **工具**：文件读写 / 搜索 / 网页抓取 / 内置浏览器 / shell / git / 子代理 / MCP / WASM 插件。
- **会话与记忆**：每会话独立落盘、重启恢复；记忆库为 2D 星图，语义 + 词法混合检索。
- **安全**：三档审批、Windows 沙箱、危险操作前拍检查点；撤销只回退本任务改过的文件。
- **扩展**：Skills、MCP、插件契约 v1；渠道 / 模型 / 角色三层配置，API Key 走 DPAPI 加密。

## 安装

从 [Releases](https://github.com/pumu21600-boop/Pawch-Agent/releases) 下载最新 Setup 运行。单文件，不依赖 Rust/Node，不要求预装 git；需要 WebView2（Windows 11 自带）。安装包未签名，SmartScreen 选「更多信息 → 仍要运行」。

## 开发

```powershell
cd web; npm install; npm run build
cd ..; cargo build --release
powershell -File scripts\build-desktop.bat
powershell -File scripts\package_release.ps1
powershell -File scripts\build-installer.ps1
```

需要 Windows + VS 2022 C++ 工具链 + Rust + Node。

## 文档

- [HTTP/SSE 接口](docs/api.md)
- [日志格式](docs/jsonl-schema.md)
- [插件契约](docs/plugin-contract-v1.md)

## License

MIT
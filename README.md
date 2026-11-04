# Pawch Agent

Windows 上的本地优先桌面 Agent。Rust（axum）+ Svelte，SSE 流式。

## 功能

- **执行循环**：模型调用工具 → 本地执行 → 结果回填，直到给出最终回答；同轮只读工具并行，写冲突自动跳过。
- **工具集**：文件读写 / 搜索 / 网页抓取 / 内置浏览器 / shell / git / 子代理 / MCP / WASM 插件。
- **会话与记忆**：会话独立落盘、重启恢复；记忆库为 2D 星图，语义 + 词法混合检索。
- **安全边界**：三档审批、Windows 沙箱、危险操作前检查点；撤销只回退本任务改过的文件。
- **扩展**：Skills、MCP、插件契约 v1；渠道 / 模型 / 角色三层配置，API Key 经 DPAPI 加密。

## 项目结构

```text
src/          Rust 服务（axum）：执行循环、工具、网关、会话、记忆、插件宿主
web/          Svelte 前端：聊天界面、设置、记忆星图、右侧面板
desktop/      Tauri 桌面壳：窗口、托盘、内置浏览器宿主
installer/    单文件 Windows 安装器
plugins/      内置插件（board / browser / usage / trace 等）
skills/       内置技能（docx / xlsx / pptx / pdf / git / github）
prompts/      系统提示词
scripts/      构建、打包、签名脚本
tests/        集成测试
evals/        评测任务
docs/         HTTP/SSE 接口、日志格式、插件契约
```

## 安装

从 [Releases](https://github.com/pumu21600-boop/Pawch-Agent/releases) 下载最新 Setup 运行。

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
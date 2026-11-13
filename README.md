<div align="center">

<img src="web/public/pawch-icon.svg" width="130" alt="Pawch Agent 图标" />

# Pawch Agent

**一个运行在 Windows 本机的桌面级 AI Agent —— 自定义模型接入，数据本地留存，任务闭环执行**

[![Release](https://img.shields.io/github/v/release/pumu21600-boop/Pawch-Agent?style=flat-square&label=Release&color=e8833a)](https://github.com/pumu21600-boop/Pawch-Agent/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/pumu21600-boop/Pawch-Agent/ci.yml?style=flat-square&label=CI)](https://github.com/pumu21600-boop/Pawch-Agent/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/pumu21600-boop/Pawch-Agent?style=flat-square&label=License&color=9c6ade)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows-0078d6?style=flat-square&logo=windows11)](https://github.com/pumu21600-boop/Pawch-Agent/releases)
[![Backend](https://img.shields.io/badge/Backend-Rust%20%2B%20axum-dea584?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Frontend](https://img.shields.io/badge/Frontend-Svelte%20%2B%20Tauri-ff3e00?style=flat-square&logo=svelte)](https://tauri.app/)

[⬇️ 下载安装包](https://github.com/pumu21600-boop/Pawch-Agent/releases) · [🚀 快速开始](#-快速开始) · [✨ 核心能力](#-核心能力) · [🧩 扩展生态](#-扩展生态) · [🛡 安全模型](#-安全模型)

</div>

---

## 📌 这是什么

**Pawch Agent 是一个装在你 Windows 电脑上的本地桌面 AI Agent**：Rust 服务 + Svelte 界面 + Tauri 桌面壳，装完就是一个桌面应用，不依赖云端服务。

你给它一句话，它把任务拆开、调用工具真正动手——读文件、改代码、跑命令、开网页、查资料、派子代理——干完带着产出回来。每一步工具调用都先过审批档位、都落在本机日志里，随时可以停、可以撤。

- **数据在本地**：会话、记忆、日志、用量台账全落在你自己的电脑上
- **模型你配**：三种协议随便接，API Key 经 Windows DPAPI 加密，不进日志、不进沙箱
- **看得见**：思考过程流式逐字可见，每次工具调用有审批卡片和完整时间线，事后可回放

> 不是聊天窗口，是把活干完的工具。

---

## ✨ 核心能力

### 🔄 执行循环

- 模型给出工具调用 → 本地执行 → 结果回填，循环到给出最终回答
- 同一轮里只读工具并行跑，写同一文件的调用自动串行，跳过原因回喂给模型自行调整
- 「停止」只停当前这一轮，会话其余部分不受影响

### 🧠 会话与记忆

- 每个会话独立落盘，刷新 / 重启自动恢复；支持分叉、置顶、归档、批量管理
- 双层记忆（全局约定 + 项目记忆）+ `AGENTS.md` 自动注入，跨会话记得住事
- 记忆库是一张 2D 星图：会话压成语义向量，与词法检索混合排序，点星即达
- 上下文接近上限自动整卷压缩，摘要与完整历史都落盘可查

### 🌐 内置浏览器

- 桌面壳里是**真实网页**（WebView2 child webview）：可以直接点、滚、填表、开 DevTools
- agent 的 browser 工具与面板共享同一实例：语义定位为主、坐标兜底，快照带稳定 ref 与增量 diff
- 用普通浏览器打开时回落为截图帧流，能力不丢

### 📄 文档技能

- 内置 docx / xlsx / pptx / pdf 技能：能读也能保真修改，产出仍是原生文件格式
- 老格式（.doc / .xls / .ppt）装有 LibreOffice 时自动转换

### 👥 子代理与定时任务

- 把子任务派给独立子代理：单独会话跑完整轮，回答回注父任务；一轮多个自动并行，花费记父任务账
- 定时任务（5 段 cron）到点自动唤醒跑一轮，完成记录可回看
- 「记忆提醒」类型：到点先翻最近对话，列出该跟进、待办的事项再动手

### 💰 用量台账

- 每次模型调用在网关单点记账：token 三路（含缓存命中）、请求 / 失败 / 延迟分位
- 费用在记账那一刻冻结，之后改价格不追溯历史账
- 全局花费上限是硬闸：到线即拒新请求，不靠自觉；面板实时显示当前对话的花费

---

## 🧩 扩展生态

对外只有四个词：**Skill / MCP / 插件 / 沙箱**。

| 扩展方式 | 一句话 |
|---|---|
| **Skill** | 一个 `SKILL.md` 就是一个技能；系统提示只带名字和一句话，模型按需拉全文 |
| **MCP** | stdio / Streamable HTTP / SSE 三种 transport，支持 OAuth 授权，工具自动挂载 |
| **插件** | 一个目录 + 一份 manifest.json：面板跑在沙箱 iframe 里，经 postMessage 白名单桥取数，manifest 没声明的权限一律拒绝 |
| **插件制作器** | 内置插件：说清需求 → 自动搭出一个能用的插件；模型自产的插件先进隔离区，转正才落地 |

---

## 🛡 安全模型

| 审批档位 | 读 | 写 | 执行（shell / git） |
|---|---|---|---|
| `read-only` | 自动 | 要批 | 要批 |
| `auto`（默认） | 自动 | 自动 | 要批 |
| `full-access` | 自动 | 自动 | 自动 |

- **执行沙箱**：Job Objects 整树击杀 + 受限 token + 环境白名单（密钥进不来）；另有 AppContainer 实验档，探测不过自动回退
- **检查点**：有风险的工具执行前自动拍影子仓库检查点；撤销只回退本任务改过的文件，绝不整树 reset
- **审批卡片**：显示风险徽章与判定依据，可按会话放行同类操作；120 秒不批视为拒绝
- **接口只认本机**：服务只接受回环来源的请求，程序化访问凭本地令牌（首启生成、仅当前用户可读）

> ⚠️ **实话实说**：审批和沙箱拦的是「模型没想清楚就动手」，不是「有人刻意攻击」——默认沙箱不隔离网络，脚本里藏的恶意行为也拦不住。请按「它真的能操作我的电脑」来配档位。

---

## 🚀 快速开始

### 方式一：安装包（推荐）

从 [**Releases**](https://github.com/pumu21600-boop/Pawch-Agent/releases) 下载 `Pawch-Agent-Setup-<版本>.exe` 双击安装，无需管理员权限。装完打开桌面壳即用，本地服务随壳自动起停。

- 需要 Windows 10 / 11（WebView2 系统自带）
- 数据默认落 `%USERPROFILE%\.pawch-agent`，默认工作区在 `<数据目录>\workspace`

### 方式二：源码构建

```powershell
git clone https://github.com/pumu21600-boop/Pawch-Agent.git
cd Pawch-Agent\web ; npm install ; npm run build   # 前端产物 web/dist，服务直接托管
cd .. ; cargo build --release                      # 或 cargo run 起开发实例
powershell -File scripts\build-installer.ps1       # 要出安装包再跑这步
```

需要 Windows + VS 2022 C++ 工具链 + Rust（MSVC）+ Node。

### 无头模式（程序化集成）

```powershell
pawch-agent.exe run "总结 docs 目录里今天改过的文件" --json
```

跑完整轮、打印最终回答，退出码 0 = 完成。定时任务、子代理、脚本集成都走这条通道。

---

## 🔌 接入模型

设置 → 模型配置：渠道 / 模型 / 角色三层解耦，改动即存即生效。

| 你有什么 | 怎么接 |
|---|---|
| DeepSeek / StepFun / GLM | 内置预设，选供应商、填 Key、拉模型列表 |
| 中转站 / 自建网关 | 选 OpenAI 兼容或 Anthropic 协议，填 Base URL + Key |
| Ollama / vLLM 等本地端点 | 走 OpenAI 兼容协议，Base URL 指到本地端口 |

- 三种协议（OpenAI 兼容 / Anthropic / Responses）自动适配，工具调用与思考回放统一处理
- 每个模型可勾选能力标记（工具 / 视觉 / 音频 / 视频），勾选优先于按名推导
- 思考强度按厂商规范适配，同一套界面里切换渠道不用改习惯

---

## 🧪 质量门禁

- CI（windows-latest）：clippy `-D warnings` + 全量 Rust 测试 + 前端单测 / 构建 + cargo-deny 供应链审计
- eval 黑盒评测：12 个任务跑真实执行循环（临时工作区隔离 + 断言引擎 + 报告）
- 本地一键等价：`scripts\run-tests.bat` / `scripts\run-clippy.bat`

---

## 📁 项目结构

```text
src/          Rust 服务（axum）：执行循环、工具、网关、会话、记忆、插件宿主、沙箱
web/          Svelte 前端：聊天界面、设置、记忆星图、右侧面板
desktop/      Tauri 桌面壳：窗口、托盘、内置浏览器宿主
installer/    单文件 Windows 安装器
plugins/      内置插件（项目文件 / 浏览器 / 记忆库 / 时间线 / 用量价格 等 9 个）
skills/       内置技能（docx / xlsx / pptx / pdf / git / github）
prompts/      系统提示词
scripts/      构建、打包、门禁脚本
tests/        集成测试
evals/        评测任务
docs/         HTTP/SSE 接口、日志格式、插件契约
```

---

## 📖 文档与参与

- [HTTP/SSE 接口](docs/api.md) —— 全部端点、方法与参数
- [会话与日志格式](docs/jsonl-schema.md) —— JSONL 事件 schema
- [插件契约 v1](docs/plugin-contract-v1.md) —— manifest 字段与桥接权限
- [更新日志](CHANGELOG.md)

发现问题欢迎提 [Issue](https://github.com/pumu21600-boop/Pawch-Agent/issues)，最好附上 `logs/agent.jsonl` 的相关行（注意先脱敏）。

---

## 📄 License

[MIT](LICENSE)

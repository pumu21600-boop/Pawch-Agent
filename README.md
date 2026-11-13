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

**Pawch Agent 是一个装在 Windows 本机的桌面级 Agent**：Rust（axum）服务 + Svelte 前端 + Tauri 桌面壳。会话、记忆、日志、用量台账全落本机；模型接你自己的渠道，Key 经 DPAPI 加密，不进日志、不进沙箱。

下面只写它做得不一样的地方，Agent 的通用概念不赘述。

> 不是聊天窗口，是把活干完的工具。

---

## ✨ 核心能力

### 🔄 执行与上下文

- 同一轮里只读工具并行，写冲突自动串行，跳过原因回喂给模型重新决策
- 上下文接近上限时在任务收尾整卷压缩，摘要与完整历史都落盘，不打断进行中的任务
- 「停止」只停当前轮，会话其余部分不动

### 🌐 内置浏览器

- 桌面壳里是 WebView2 child webview：右侧面板的网页可以直接点、滚、填表、开 DevTools，agent 的 browser 工具操作的是同一个实例
- 语义定位为主、坐标兜底，快照带稳定 ref 与增量 diff，页面哪次操作改了什么看得见
- 没有桌面壳、用普通浏览器打开时，回落为截图帧流

### 🧠 记忆

- 记忆库是一张 2D 星图：会话压成向量落进星图，语义与词法混合排序，点星看详情
- 全局约定与项目记忆两层存放，`AGENTS.md` 每轮自动注入

### 📄 文档

- docx / xlsx / pptx / pdf 能读也能保真改，产出仍是原生文件格式，不是转成纯文本再转回去
- 老格式（.doc / .xls / .ppt）装了 LibreOffice 时自动转换

### 👥 子代理与定时

- 子任务派给独立子代理：单独会话跑完整轮、回答回注父任务，一轮多个自动并行，花费记进父任务账
- 定时任务到点自动唤醒跑一轮，完成记录可回看；「记忆提醒」类型会先翻最近对话，列出待跟进的事项

### 💰 台账

- 每次模型调用在网关单点记账：token 分输入 / 缓存 / 输出三路，请求、失败、延迟分位全落账
- 费用记账即冻结，之后改价不追溯历史账；全局花费上限到线即拒新请求，主对话与辅助调用同一道闸

---

## 🧩 扩展生态

对外只有四个词：**Skill / MCP / 插件 / 沙箱**。

| 扩展方式 | 一句话 |
|---|---|
| **Skill** | 一个 `SKILL.md` 就是一个技能；系统提示只带名字和一句话，模型按需拉全文 |
| **MCP** | stdio / Streamable HTTP / SSE 三种 transport，支持 OAuth 授权 |
| **插件** | 一个目录 + 一份 manifest.json；面板跑在沙箱 iframe 里经白名单桥取数，manifest 没声明的权限一律拒绝 |
| **插件制作器** | 内置插件：说清需求就自动搭一个能用的；模型自产的先进隔离区，转正才落盘 |

---

## 🛡 安全模型

| 审批档位 | 读 | 写 | 执行（shell / git） |
|---|---|---|---|
| `read-only` | 自动 | 要批 | 要批 |
| `auto`（默认） | 自动 | 自动 | 要批 |
| `full-access` | 自动 | 自动 | 自动 |

- 执行沙箱：Job Objects 整树击杀 + 受限令牌 + 环境变量白名单，密钥进不了沙箱；AppContainer 实验档探测不过自动回退
- 检查点：风险操作前自动拍影子仓库检查点，撤销只回退本任务改过的文件，绝不整树 reset
- 审批卡片：带风险徽章与判定依据，120 秒不批视为拒绝，可按会话放行同类操作
- 服务只接受本机回环请求，程序化访问凭本地令牌（首启生成、仅当前用户可读）

> ⚠️ 审批和沙箱拦的是误操作，不是刻意攻击：默认沙箱不隔离网络，脚本里藏的恶意行为也拦不住。按「它真的能操作我的电脑」来配档位。

---

## 📸 界面预览

**聊天 + 内置浏览器**：agent 正在迭代一个 3D 场景页——工具调用逐条落卡，右侧是壳内真实网页，不是截图贴图。

![聊天与内置浏览器](docs/assets/ui-chat.jpg)

**记忆库星图**：每个会话压成一颗星，语义 + 词法混合检索，点星看详情。

![记忆库星图](docs/assets/ui-galaxy.jpg)

**用量台账**：缓存命中率、首 token 延迟、输出速度、费用，网关单点记账。

![用量台账](docs/assets/ui-usage.png)

---

## 🚀 快速开始

### 方式一：安装包（推荐）

从 [**Releases**](https://github.com/pumu21600-boop/Pawch-Agent/releases) 下载 `Pawch-Agent-Setup-<版本>.exe`，双击安装，无需管理员权限（需要 Windows 10 / 11，WebView2 系统自带）。数据默认落 `%USERPROFILE%\.pawch-agent`，工作区在 `<数据目录>\workspace`。

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

- 三种协议（OpenAI 兼容 / Anthropic / Responses）自动适配，工具调用与思考回放走同一套处理
- 模型能力标记（工具 / 视觉 / 音频 / 视频）手动勾选优先于按名推导

---

## 🧪 质量门禁

- CI（windows-latest）四条作业：clippy `-D warnings`、全量 Rust 测试、前端单测 + 构建 + 静态门禁、cargo-deny 供应链审计
- 其中一条静态门禁专抓「fetch 之后忘接 `.json()`」——这个 bug 在本仓复发过两次，两次都是界面静默显示空值
- eval：12 个任务跑真实执行循环，临时工作区隔离，断言引擎出报告；本地等价入口 `scripts\run-tests.bat` / `scripts\run-clippy.bat`

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

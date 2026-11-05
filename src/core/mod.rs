//! 核心循环层（刚性）—— 总架构 §4
//!
//! 写死链路：任务解析 → 调工具 → 结果回填 → 状态判定 → 终止。
//! 实现 agent.rs 模型驱动循环；并行执行 / 写冲突检测也在该模块内。

pub mod agent;
pub mod cancel;
pub mod rules;
pub mod steer;
pub mod subagent;

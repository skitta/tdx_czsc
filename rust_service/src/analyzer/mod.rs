//! 分析快照编排（rust-server 层）。
//!
//! 职责：请求校验、组装分析管线（RawBar → CZSC 分型/笔 → 中枢/段/买卖点
//! → 通道与对象映射 → `AnalysisSnapshot`）、参数解析（请求选项回退到配置）。
//!
//! 模块划分：
//! - `rawbars`：请求 → RawBar 转换（频率解析、量能来源链、时间轴合成）；
//! - `pivots`：中枢构建与三买/三卖严格化；
//! - `segs`：线段识别（特征序列分型法）；
//! - `signals`：401~406 买卖点信号；
//! - `channels`：func_id → 对齐 DataLen 的 float[] 输出通道；
//! - `objects`：分型/笔/中枢/段 → proto 对象映射与 K 线下标对齐；
//! - `settings`：请求参数解析与窗口设置；
//! - `snapshot`：快照装配与剖面计时；
//! - `pipeline`：纯全量入口与增量/全量用例编排。

mod channels;
mod error;
mod incremental;
mod objects;
mod pipeline;
mod pivots;
mod rawbars;
mod segs;
mod settings;
mod signals;
mod snapshot;

#[cfg(test)]
mod fixtures;

pub use channels::{ChannelBuilder, ChannelSpec};
pub use error::AnalysisError;
pub use pipeline::SnapshotAnalyzer;
pub(crate) use pipeline::{AnalysisEngine, IncrementalMode};
pub use pivots::PivotAnalyzer;
pub use rawbars::{FreqHint, RawBarFactory};
pub(crate) use settings::ResolvedParams;
pub use settings::{AnalysisParams, AnalysisWindow};
pub use signals::{BuySellPoint, FastSignalEngine, MaSeriesCache, SignalAnalyzer};
pub use snapshot::SnapshotProfile;

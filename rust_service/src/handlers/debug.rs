//! 响应装饰：调试通道（901~903）补丁与耗时统计构造。
//!
//! 调试通道由服务端在响应前回填：901 缓存命中、902 服务就绪、903 最近
//! 一次耗时（毫秒）。耗时统计在缓存命中/全量分析两条路径上构造，
//! 这里集中定义避免多处重复。

use crate::proto::analysis::{AnalysisSnapshot, ChannelSeries, TimingStats};

/// 调试通道编号（与 `analysis.proto` 的 901~903 一致）。
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DebugChannel {
    /// 901 缓存命中：请求未经等待/计算直接读到既有缓存填 1，否则 0。
    CacheHit = 901,
    /// 902 服务就绪：恒为 1。
    ServiceReady = 902,
    /// 903 本次请求总耗时（毫秒）。
    LatencyMs = 903,
}

impl DebugChannel {
    fn func_id(self) -> u32 {
        self as u32
    }

    /// 把该通道的全部取值回填为 `value`（通道不存在时无操作）。
    fn fill(self, channels: &mut [ChannelSeries], value: f32) {
        for channel in channels.iter_mut().filter(|c| c.func_id == self.func_id()) {
            channel.values.fill(value);
        }
    }
}

/// 缓存命中状态（决定 901 的取值）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheState {
    /// 请求直接读到既有缓存（901=1）。
    Hit,
    /// 请求经过等待或自行计算（901=0）。
    Miss,
}

impl CacheState {
    fn debug_value(self) -> f32 {
        match self {
            Self::Hit => 1.0,
            Self::Miss => 0.0,
        }
    }
}

/// 调试通道补丁：快照响应前统一回填 901~903。
pub(crate) struct SnapshotPatch {
    pub(crate) cache: CacheState,
    pub(crate) latency_ms: f32,
}

impl SnapshotPatch {
    /// 应用补丁（通道缺失时静默跳过）。
    pub(crate) fn apply(&self, snapshot: &mut AnalysisSnapshot) {
        DebugChannel::CacheHit.fill(&mut snapshot.channels, self.cache.debug_value());
        DebugChannel::ServiceReady.fill(&mut snapshot.channels, 1.0);
        DebugChannel::LatencyMs.fill(&mut snapshot.channels, self.latency_ms);
    }
}

/// 耗时统计构造：缓存命中路径 `analyze_ms = 0`，计算路径两者相等。
pub(crate) struct Timing;

impl Timing {
    pub(crate) fn cached(total_ms: u32) -> TimingStats {
        TimingStats {
            total_ms,
            analyze_ms: 0,
        }
    }

    pub(crate) fn computed(total_ms: u32) -> TimingStats {
        TimingStats {
            total_ms,
            analyze_ms: total_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(func_id: u32) -> ChannelSeries {
        ChannelSeries {
            func_id,
            kind: 0,
            name: String::new(),
            values: vec![0.0; 4],
        }
    }

    #[test]
    fn snapshot_patch_fills_901_902_903() {
        let mut snapshot = AnalysisSnapshot {
            channels: vec![channel(101), channel(901), channel(902), channel(903)],
            ..Default::default()
        };
        SnapshotPatch {
            cache: CacheState::Hit,
            latency_ms: 1.5,
        }
        .apply(&mut snapshot);

        assert_eq!(
            snapshot.channels[0].values,
            vec![0.0; 4],
            "非调试通道不受影响"
        );
        assert_eq!(snapshot.channels[1].values, vec![1.0; 4]);
        assert_eq!(snapshot.channels[2].values, vec![1.0; 4]);
        assert_eq!(snapshot.channels[3].values, vec![1.5; 4]);
    }

    #[test]
    fn timing_matches_cache_and_compute_paths() {
        assert_eq!(
            Timing::cached(7),
            TimingStats {
                total_ms: 7,
                analyze_ms: 0
            }
        );
        assert_eq!(
            Timing::computed(7),
            TimingStats {
                total_ms: 7,
                analyze_ms: 7
            }
        );
    }
}

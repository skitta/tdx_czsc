//! 响应装饰：调试通道（901~903）补丁与耗时统计构造。
//!
//! 调试通道由服务端在响应前回填：901 缓存命中、902 服务就绪、903 最近
//! 一次耗时（毫秒）。耗时统计在缓存命中/全量分析两条路径上构造，
//! 这里集中定义避免三处重复。

use crate::proto::analysis::{ChannelSeries, TimingStats};

/// 调试通道补丁：按 func_id 回填缓存命中/耗时值。
///
/// - 901 `debug_cache_hit`：命中缓存填 1，全量分析填 0；
/// - 902 `debug_service_ready`：恒填 1；
/// - 903 `debug_latency_ms`：填本次请求总耗时。
pub(crate) fn patch_debug_channels(
    channels: &mut [ChannelSeries],
    cache_hit: bool,
    latency_ms: f32,
) {
    for channel in channels {
        match channel.func_id {
            901 => channel.values.fill(if cache_hit { 1.0 } else { 0.0 }),
            902 => channel.values.fill(1.0),
            903 => channel.values.fill(latency_ms),
            _ => {}
        }
    }
}

/// 构造耗时统计（缓存命中路径 analyze_ms=0；全量分析路径两者相等）。
pub(crate) fn timing_stats(total_ms: u32, analyze_ms: u32) -> TimingStats {
    TimingStats {
        total_ms,
        build_bars_ms: 0,
        analyze_ms,
        map_channels_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_debug_channels_fills_901_902_903() {
        let mut channels = vec![
            ChannelSeries {
                func_id: 901,
                kind: 0,
                name: String::new(),
                values: vec![0.0; 4],
            },
            ChannelSeries {
                func_id: 902,
                kind: 0,
                name: String::new(),
                values: vec![0.0; 4],
            },
            ChannelSeries {
                func_id: 903,
                kind: 0,
                name: String::new(),
                values: vec![0.0; 4],
            },
        ];
        patch_debug_channels(&mut channels, true, 1.5);
        assert_eq!(channels[0].values, vec![1.0; 4]);
        assert_eq!(channels[1].values, vec![1.0; 4]);
        assert_eq!(channels[2].values, vec![1.5; 4]);
    }
}
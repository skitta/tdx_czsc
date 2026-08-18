use std::time::Instant;

use anyhow::Result;

use crate::analyzer;
use crate::cache::CacheKey;
use crate::proto::analysis::{AnalyzeRequest, AnalyzeResponse, ChannelSeries, TimingStats};
use crate::state::AppState;

pub async fn handle(
    state: &std::sync::Arc<AppState>,
    request: AnalyzeRequest,
) -> Result<AnalyzeResponse> {
    let started = Instant::now();
    let _guard = ActiveRequestGuard::new(state.clone());
    let options = request.options.clone().unwrap_or_default();

    let cache_key = CacheKey {
        func_group: request.func_group.clone(),
        input_hash: request.input_hash,
        data_len: request.data_len,
        freq_hint: request.freq_hint.clone(),
        max_bi_num: if options.max_bi_num > 0 {
            options.max_bi_num
        } else {
            state.config.max_bi_num
        },
        min_bi_len: if options.min_bi_len > 0 {
            options.min_bi_len
        } else {
            state.config.min_bi_len
        },
        analysis_window: if options.analysis_window > 0 {
            options.analysis_window
        } else {
            state.config.default_analysis_window
        },
    };

    if request.prefer_cached {
        let cache = state.cache.read().await;
        if let Some(mut snapshot) = cache.get(&cache_key) {
            patch_debug_channels(&mut snapshot.channels, true, 0.0);
            return Ok(AnalyzeResponse {
                snapshot: Some(snapshot),
                service_cache_hit: true,
                timing: Some(TimingStats {
                    total_ms: started.elapsed().as_millis() as u32,
                    build_bars_ms: 0,
                    analyze_ms: 0,
                    map_channels_ms: 0,
                }),
            });
        }
    }

    let mut snapshot = analyzer::build_snapshot(&request, state)?;
    let elapsed_ms = started.elapsed().as_millis() as u32;
    patch_debug_channels(&mut snapshot.channels, false, elapsed_ms as f32);

    {
        let mut cache = state.cache.write().await;
        cache.insert(cache_key, snapshot.clone());
    }

    Ok(AnalyzeResponse {
        snapshot: Some(snapshot),
        service_cache_hit: false,
        timing: Some(TimingStats {
            total_ms: elapsed_ms,
            build_bars_ms: 0,
            analyze_ms: elapsed_ms,
            map_channels_ms: 0,
        }),
    })
}

fn patch_debug_channels(channels: &mut [ChannelSeries], cache_hit: bool, latency_ms: f32) {
    for channel in channels {
        match channel.func_id {
            901 => channel.values.fill(if cache_hit { 1.0 } else { 0.0 }),
            902 => channel.values.fill(1.0),
            903 => channel.values.fill(latency_ms),
            _ => {}
        }
    }
}

struct ActiveRequestGuard {
    state: std::sync::Arc<AppState>,
}

impl ActiveRequestGuard {
    fn new(state: std::sync::Arc<AppState>) -> Self {
        state
            .active_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { state }
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.state
            .active_requests
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};
    use crate::state::AppState;

    fn sample_request(prefer_cached: bool) -> AnalyzeRequest {
        let close = vec![
            10.0, 10.2, 10.5, 10.1, 9.8, 9.6, 9.9, 10.3, 10.7, 10.4, 10.1, 9.7, 9.5, 9.9, 10.4,
            10.9, 10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.8, 11.2, 10.9, 10.5, 10.0, 9.6, 9.9, 10.3,
            10.7, 11.0, 10.8, 10.3, 9.9, 9.5, 9.8, 10.1, 10.6, 11.1, 10.7, 10.2, 9.7, 9.3, 9.6,
            10.0, 10.5, 10.9, 10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.6, 11.0, 10.8, 10.4, 10.0, 9.7,
            10.0, 10.4, 10.9, 11.3,
        ];
        let mut high = Vec::with_capacity(close.len());
        let mut low = Vec::with_capacity(close.len());
        for (i, price) in close.iter().enumerate() {
            high.push(*price + 0.25 + (i % 3) as f32 * 0.03);
            low.push(*price - 0.22 - (i % 2) as f32 * 0.02);
        }

        AnalyzeRequest {
            func_group: "single_level_structure".to_string(),
            requested_func_ids: vec![101, 901, 902, 903],
            symbol_hint: "000001.SZ".to_string(),
            freq_hint: "F30".to_string(),
            data_len: close.len() as u32,
            input_hash: 0x22334455,
            bar_build_mode: crate::proto::analysis::BarBuildMode::BarBuildFromHlc3 as i32,
            series: Some(MarketSeries {
                open: Vec::new(),
                high,
                low,
                close,
                vol: Vec::new(),
                amount: Vec::new(),
                dt_unix_ms: Vec::new(),
            }),
            options: Some(CzscOptions {
                max_bi_num: 50,
                min_bi_len: 3,
                analysis_window: 64,
                include_channels: true,
                include_objects: false,
                enable_incremental: false,
            }),
            prefer_cached,
            caller_instance_id: 99,
        }
    }

    fn state() -> std::sync::Arc<AppState> {
        let mut config = AppConfig::default();
        config.min_bi_len = 3;
        config.default_analysis_window = 64;
        AppState::new(config)
    }

    fn channel_values(response: &AnalyzeResponse, func_id: u32) -> &Vec<f32> {
        &response
            .snapshot
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .find(|x| x.func_id == func_id)
            .unwrap_or_else(|| panic!("channel {func_id} not found"))
            .values
    }

    #[tokio::test]
    async fn handle_patches_debug_channels_and_hits_cache() {
        let state = state();

        let first = handle(&state, sample_request(false))
            .await
            .expect("first analyze should succeed");
        assert!(!first.service_cache_hit);
        assert!(channel_values(&first, 901).iter().all(|x| *x == 0.0));
        assert!(channel_values(&first, 902).iter().all(|x| *x == 1.0));
        assert!(channel_values(&first, 903).iter().any(|x| *x >= 0.0));

        let second = handle(&state, sample_request(true))
            .await
            .expect("second analyze should succeed");
        assert!(second.service_cache_hit);
        assert!(channel_values(&second, 901).iter().all(|x| *x == 1.0));
        assert!(channel_values(&second, 902).iter().all(|x| *x == 1.0));
    }
}

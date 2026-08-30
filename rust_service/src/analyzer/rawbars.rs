//! 请求 → RawBar 转换（纯函数，无运行时状态依赖）。
//!
//! 负责：频率解析（`parse_freq`）、从 `AnalyzeRequest` 窗口切片构造 czsc
//! `RawBar` 序列（`build_raw_bars`）、量能三级来源链（真实 vol / 振幅代理
//! / 0）、时间轴解析（真实 `dt_unix_ms` 或按频率合成）。

use chrono::{DateTime, Duration, TimeZone, Utc};
use czsc_core::objects::bar::{RawBar, RawBarBuilder};
use czsc_core::objects::freq::Freq;
use std::str::FromStr;

use crate::analyzer::error::AnalysisError;
use crate::proto::analysis::{AnalyzeRequest, MarketSeries};

/// 解析通达信/调用方传入的周期提示字符串。
///
/// 空串回退到 30 分钟；支持形如 `D`、`1d`、`30m`、`F30` 等别名
/// （`normalize_freq_hint` 转中文名后再交给 czsc 解析）。
pub fn parse_freq(freq_hint: &str) -> Result<Freq, AnalysisError> {
    if freq_hint.is_empty() {
        return Ok(Freq::F30);
    }

    Freq::from_str(freq_hint)
        .or_else(|_| Freq::from_str(&normalize_freq_hint(freq_hint)))
        .map_err(|_| AnalysisError::UnsupportedFreq(freq_hint.to_string()))
}

fn normalize_freq_hint(freq_hint: &str) -> String {
    match freq_hint {
        "F1" | "1m" => "1分钟".to_string(),
        "F5" | "5m" => "5分钟".to_string(),
        "F15" | "15m" => "15分钟".to_string(),
        "F30" | "30m" => "30分钟".to_string(),
        "F60" | "60m" => "60分钟".to_string(),
        "F120" | "120m" => "120分钟".to_string(),
        "F240" | "240m" => "240分钟".to_string(),
        "d" | "1d" | "D" => "日线".to_string(),
        "w" | "1w" | "W" => "周线".to_string(),
        "m" | "1M" | "M" => "月线".to_string(),
        other => other.to_string(),
    }
}

/// 从请求的行情序列构造 czsc `RawBar` 列表（分析窗口切片）。
pub fn build_raw_bars(
    request: &AnalyzeRequest,
    freq: Freq,
    window_start: usize,
    derive_volume_proxy: bool,
) -> Result<Vec<RawBar>, AnalysisError> {
    let series = request
        .series
        .as_ref()
        .ok_or_else(|| AnalysisError::invalid_request("market series is missing"))?;

    let symbol = if request.symbol_hint.is_empty() {
        "TDX".to_string()
    } else {
        request.symbol_hint.clone()
    };

    let mut bars = Vec::with_capacity(request.data_len as usize - window_start);
    for i in window_start..request.data_len as usize {
        let dt = resolve_bar_datetime(i, &series.dt_unix_ms, freq)?;
        let close = series.close[i] as f64;
        let open = series.open.get(i).copied().unwrap_or(series.close[i]) as f64;
        // 量能来源链（背驰判定用，见 signals.rs 模块说明）：
        // 1. 请求携带的真实 vol（950 量能通道 / CSV 工具链）——原样使用；
        // 2. vol 缺失且开启代理 → 单根相对振幅 (high-low)/close*100，
        //    恢复"量能背驰"维度（通达信公式未挂 950 通道时的兜底）；
        // 3. 否则 0.0（现状行为，量能背驰恒不成立）。
        let vol = if series.vol.is_empty() {
            if derive_volume_proxy {
                amplitude_proxy_vol(series, i)
            } else {
                0.0
            }
        } else {
            series.vol[i] as f64
        };
        let amount = series.amount.get(i).copied().unwrap_or(0.0) as f64;

        let bar = RawBarBuilder::default()
            .symbol(symbol.clone())
            .dt(dt)
            .id(i as i32)
            .freq(freq)
            .open(open)
            .close(close)
            .high(series.high[i] as f64)
            .low(series.low[i] as f64)
            .vol(vol)
            .amount(amount)
            .build()
            .map_err(|e| AnalysisError::invalid_bar_data(i, e.to_string()))?;

        bars.push(bar);
    }

    Ok(bars)
}

/// 单根K线相对振幅（%）：(high - low) / close × 100。
///
/// 量能代理（通达信公式未挂 950 量能通道时的兜底）：无量纲、不随长期价格
/// 尺度漂移，与 czsc 量能力度"笔内累加、同窗口横向比较"的语义兼容。
/// `close <= 0`（异常数据）时返回 0.0，保证无 NaN/Inf。
fn amplitude_proxy_vol(series: &MarketSeries, index: usize) -> f64 {
    let close = series.close[index] as f64;
    if close <= 0.0 {
        return 0.0;
    }
    let amplitude = (series.high[index] - series.low[index]) as f64;
    (amplitude / close * 100.0).max(0.0)
}

fn resolve_bar_datetime(
    index: usize,
    provided: &[i64],
    freq: Freq,
) -> Result<DateTime<Utc>, AnalysisError> {
    if let Some(ts_ms) = provided.get(index) {
        return Utc.timestamp_millis_opt(*ts_ms).single().ok_or_else(|| {
            AnalysisError::invalid_bar_data(index, format!("invalid dt_unix_ms: {ts_ms}"))
        });
    }

    let base = Utc
        .with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
        .single()
        .ok_or_else(|| {
            AnalysisError::invalid_bar_data(index, "failed to create synthetic base datetime")
        })?;

    Ok(base + synthetic_freq_step(freq, index as i64))
}

fn synthetic_freq_step(freq: Freq, index: i64) -> Duration {
    if let Some(minutes) = freq.minutes() {
        return Duration::minutes(minutes * index);
    }

    match freq {
        Freq::D => Duration::days(index),
        Freq::W => Duration::weeks(index),
        Freq::M => Duration::days(30 * index),
        Freq::S => Duration::days(90 * index),
        Freq::Y => Duration::days(365 * index),
        _ => Duration::minutes(index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::analysis::CzscOptions;

    fn sample_request() -> AnalyzeRequest {
        let close: Vec<f32> = (0..64).map(|i| 10.0 + (i % 7) as f32 * 0.5).collect();
        let high = close.iter().map(|c| c + 0.2).collect();
        let low = close.iter().map(|c| c - 0.2).collect();
        AnalyzeRequest {
            func_group: "single_level_structure".to_string(),
            requested_func_ids: vec![101],
            symbol_hint: "TEST".to_string(),
            freq_hint: "F30".to_string(),
            data_len: close.len() as u32,
            input_hash: 0,
            series: Some(MarketSeries {
                open: Vec::new(),
                high,
                low,
                close,
                vol: Vec::new(),
                amount: Vec::new(),
                dt_unix_ms: Vec::new(),
            }),
            options: Some(CzscOptions::default()),
            prefer_cached: false,
        }
    }

    #[test]
    fn parse_freq_accepts_short_aliases() {
        assert_eq!(parse_freq("30m").unwrap(), Freq::F30);
        assert_eq!(parse_freq("D").unwrap(), Freq::D);
        assert_eq!(parse_freq("1d").unwrap(), Freq::D);
        assert_eq!(parse_freq("").unwrap(), Freq::F30);
        assert!(parse_freq("nonsense").is_err());
    }

    #[test]
    fn build_raw_bars_slices_window_and_passes_series() {
        let request = sample_request();
        let bars = build_raw_bars(&request, Freq::F30, 10, true).expect("bars");
        assert_eq!(bars.len(), (request.data_len - 10) as usize);
        for (k, bar) in bars.iter().enumerate() {
            let i = k + 10;
            assert_eq!(bar.id, i as i32);
            assert_eq!(bar.high, request.series.as_ref().unwrap().high[i] as f64);
        }
    }

    /// 量能来源链 1：请求携带真实 vol（950 量能通道 / CSV 工具链）→ 原样使用。
    #[test]
    fn build_raw_bars_uses_provided_volume() {
        let mut request = sample_request();
        let expected_vol: Vec<f32> = (0..request.data_len)
            .map(|i| (i % 5) as f32 * 100.0)
            .collect();
        request.series.as_mut().unwrap().vol = expected_vol.clone();

        let bars = build_raw_bars(&request, Freq::F30, 0, true).expect("bars");
        for (i, bar) in bars.iter().enumerate() {
            assert_eq!(
                bar.vol, expected_vol[i] as f64,
                "vol at {i} must pass through"
            );
        }
    }

    /// 量能来源链 2：vol 缺失 + 代理开启 → 每根 (high-low)/close*100。
    #[test]
    fn build_raw_bars_derives_amplitude_proxy_when_vol_absent() {
        let request = sample_request();
        let series = request.series.as_ref().unwrap();
        let bars = build_raw_bars(&request, Freq::F30, 0, true).expect("bars");
        for (i, bar) in bars.iter().enumerate() {
            let close = series.close[i] as f64;
            let expected = (series.high[i] - series.low[i]) as f64 / close * 100.0;
            assert!(
                (bar.vol - expected).abs() < 1e-6,
                "proxy vol at {i}: got {} expected {}",
                bar.vol,
                expected
            );
        }
    }

    /// 量能来源链 3：vol 缺失 + 代理关闭 → 0.0（现状行为，可回退）。
    #[test]
    fn build_raw_bars_zero_when_proxy_disabled() {
        let request = sample_request();
        let bars = build_raw_bars(&request, Freq::F30, 0, false).expect("bars");
        assert!(bars.iter().all(|bar| bar.vol == 0.0));
    }
}

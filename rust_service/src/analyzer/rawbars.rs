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

/// 行情序列形状校验：所有非空序列都必须与 `data_len` 对齐。
pub(crate) trait SeriesShape {
    fn validate_lengths(&self, data_len: u32) -> Result<(), AnalysisError>;
}

impl SeriesShape for MarketSeries {
    fn validate_lengths(&self, data_len: u32) -> Result<(), AnalysisError> {
        let expected = data_len as usize;

        let check = |name: &str, values: &[f32], required: bool| -> Result<(), AnalysisError> {
            if values.is_empty() {
                if required {
                    return Err(AnalysisError::invalid_request(format!(
                        "required series {name} is empty"
                    )));
                }
                return Ok(());
            }
            if values.len() != expected {
                return Err(AnalysisError::invalid_request(format!(
                    "series {name} length {} != data_len {expected}",
                    values.len()
                )));
            }
            Ok(())
        };

        check("high", &self.high, true)?;
        check("low", &self.low, true)?;
        check("close", &self.close, true)?;
        check("open", &self.open, false)?;
        check("vol", &self.vol, false)?;
        check("amount", &self.amount, false)?;
        if !self.dt_unix_ms.is_empty() && self.dt_unix_ms.len() != expected {
            return Err(AnalysisError::invalid_request(format!(
                "series dt_unix_ms length {} != data_len {expected}",
                self.dt_unix_ms.len()
            )));
        }

        Ok(())
    }
}

/// 周期提示字符串的解析入口。
pub struct FreqHint<'a>(&'a str);

impl<'a> FreqHint<'a> {
    pub fn new(value: &'a str) -> Self {
        Self(value)
    }

    /// 空串回退到 30 分钟；支持 `D`、`1d`、`30m`、`F30` 等别名。
    pub fn parse(self) -> Result<Freq, AnalysisError> {
        if self.0.is_empty() {
            return Ok(Freq::F30);
        }

        Freq::from_str(self.0)
            .or_else(|_| Freq::from_str(&Self::normalize(self.0)))
            .map_err(|_| AnalysisError::UnsupportedFreq(self.0.to_string()))
    }

    fn normalize(value: &str) -> String {
        match value {
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
}

/// 从请求的行情序列构造 czsc `RawBar` 列表（分析窗口切片）。
pub struct RawBarFactory<'a> {
    request: &'a AnalyzeRequest,
    derive_volume_proxy: bool,
}

impl<'a> RawBarFactory<'a> {
    pub fn new(request: &'a AnalyzeRequest, derive_volume_proxy: bool) -> Self {
        Self {
            request,
            derive_volume_proxy,
        }
    }

    pub fn build(&self, freq: Freq, window_start: usize) -> Result<Vec<RawBar>, AnalysisError> {
        let series = self
            .request
            .series
            .as_ref()
            .ok_or_else(|| AnalysisError::invalid_request("market series is missing"))?;
        series.validate_lengths(self.request.data_len)?;
        let data_len = self.request.data_len as usize;
        if window_start > data_len {
            return Err(AnalysisError::invalid_request(format!(
                "window start {window_start} exceeds data_len {data_len}"
            )));
        }

        let mut bars = Vec::with_capacity(data_len - window_start);
        for i in window_start..data_len {
            let dt = self.resolve_bar_datetime(i, &series.dt_unix_ms, freq)?;
            let close = series.close[i] as f64;
            let open = series.open.get(i).copied().unwrap_or(series.close[i]) as f64;
            // 量能来源链：真实 vol → 振幅代理 → 0。
            let vol = if series.vol.is_empty() {
                if self.derive_volume_proxy {
                    self.amplitude_proxy_vol(series, i)
                } else {
                    0.0
                }
            } else {
                series.vol[i] as f64
            };
            let amount = series.amount.get(i).copied().unwrap_or(0.0) as f64;

            let bar = RawBarBuilder::default()
                .symbol("TDX")
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
    fn amplitude_proxy_vol(&self, series: &MarketSeries, index: usize) -> f64 {
        let close = series.close[index] as f64;
        if close <= 0.0 {
            return 0.0;
        }
        let amplitude = (series.high[index] - series.low[index]) as f64;
        (amplitude / close * 100.0).max(0.0)
    }

    fn resolve_bar_datetime(
        &self,
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

        Ok(base + Self::synthetic_freq_step(freq, index as i64))
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
        assert_eq!(FreqHint::new("30m").parse().unwrap(), Freq::F30);
        assert_eq!(FreqHint::new("D").parse().unwrap(), Freq::D);
        assert_eq!(FreqHint::new("1d").parse().unwrap(), Freq::D);
        assert_eq!(FreqHint::new("").parse().unwrap(), Freq::F30);
        assert!(FreqHint::new("nonsense").parse().is_err());
    }

    #[test]
    fn build_raw_bars_slices_window_and_passes_series() {
        let request = sample_request();
        let bars = RawBarFactory::new(&request, true)
            .build(Freq::F30, 10)
            .expect("bars");
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

        let bars = RawBarFactory::new(&request, true)
            .build(Freq::F30, 0)
            .expect("bars");
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
        let bars = RawBarFactory::new(&request, true)
            .build(Freq::F30, 0)
            .expect("bars");
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
        let bars = RawBarFactory::new(&request, false)
            .build(Freq::F30, 0)
            .expect("bars");
        assert!(bars.iter().all(|bar| bar.vol == 0.0));
    }
}

//! 分析管线：纯全量入口与增量/全量用例编排。
//!
//! [`SnapshotAnalyzer`] 只负责“给定参数，完整算一次”；[`AnalysisEngine`]
//! 在其上增加增量承接与回退，是 handler 的直接下游。

use std::time::Instant;

use czsc_core::objects::bar::RawBar;

use crate::proto::analysis::{AnalysisSnapshot, AnalyzeRequest, CzscOptions};

use super::error::AnalysisError;
use super::incremental::{AcceptOutcome, IncrementalKey, IncrementalState, IncrementalStore};
use super::rawbars::RawBarFactory;
use super::settings::{AnalysisParams, AnalysisSettings};
use super::signals::{EvolutionEngine, EvolutionOutcome};
use super::snapshot::{SnapshotAssembler, SnapshotProfile};

/// 纯全量分析入口：examples、集成测试和全量回退共用。
pub struct SnapshotAnalyzer<'a> {
    params: &'a AnalysisParams,
}

impl<'a> SnapshotAnalyzer<'a> {
    pub fn new(params: &'a AnalysisParams) -> Self {
        Self { params }
    }

    pub fn build_snapshot(
        &self,
        request: &AnalyzeRequest,
    ) -> Result<AnalysisSnapshot, AnalysisError> {
        self.build_snapshot_profiled(request, None)
    }

    pub fn build_snapshot_profiled(
        &self,
        request: &AnalyzeRequest,
        mut profile: Option<&mut SnapshotProfile>,
    ) -> Result<AnalysisSnapshot, AnalysisError> {
        let settings = AnalysisSettings::new(request, self.params)?;
        let bars = self.raw_bars(&settings, profile.as_deref_mut())?;
        self.analyze_with_settings(&settings, &bars, profile)
            .map(|(snapshot, _)| snapshot)
    }

    pub(crate) fn raw_bars(
        &self,
        settings: &AnalysisSettings<'_>,
        profile: Option<&mut SnapshotProfile>,
    ) -> Result<Vec<RawBar>, AnalysisError> {
        let started = Instant::now();
        let bars = RawBarFactory::new(settings.request(), settings.params().derive_volume_proxy)
            .build(settings.freq(), settings.window_start())?;
        if let Some(profile) = profile {
            profile.rawbars += started.elapsed();
        }
        Ok(bars)
    }

    /// 用已校验设置与窗口 K 线执行一次完整演化 + 装配。
    pub(crate) fn analyze_with_settings(
        &self,
        settings: &AnalysisSettings<'_>,
        bars: &[RawBar],
        mut profile: Option<&mut SnapshotProfile>,
    ) -> Result<(AnalysisSnapshot, EvolutionOutcome), AnalysisError> {
        if bars.is_empty() {
            return Err(AnalysisError::invalid_request(
                "bars are empty after applying analysis window",
            ));
        }

        let started = Instant::now();
        let mut outcome = EvolutionEngine::new(
            bars,
            settings.max_bi_num() as usize,
            settings.min_bi_len() as usize,
            settings.wants_signals(),
        )
        .run_profiled(profile.as_deref_mut());
        if let Some(profile) = profile.as_deref_mut() {
            profile.evolve += started.elapsed();
        }

        let points = match outcome.evaluator.as_mut() {
            Some(evaluator) => evaluator.finalize_points(&outcome.czsc),
            None => Vec::new(),
        };
        let snapshot = SnapshotAssembler::new(settings, &outcome.czsc, points).assemble(profile)?;
        Ok((snapshot, outcome))
    }
}

/// 分析演进方式：增量承接或全量重算（只有全量计入 `total_analyses`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evolution {
    Incremental,
    Full,
}

impl Evolution {
    pub(crate) fn is_full(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// 一次分析的产物：请求通道快照 + 演进方式。
pub(crate) struct AnalysisOutcome {
    pub(crate) snapshot: AnalysisSnapshot,
    pub(crate) evolution: Evolution,
}

/// 分析用例服务：持有增量状态缓存，决定本次请求走增量还是全量。
#[derive(Debug)]
pub(crate) struct AnalysisEngine {
    store: IncrementalStore,
}

/// 增量开关：决定本次请求是否复用增量状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IncrementalMode {
    /// 每次都完整演化，不复用增量状态。
    Full,
    /// 尝试增量承接，承接失败时回退全量。
    Incremental,
}

impl IncrementalMode {
    /// 配置开启，或请求显式要求时启用增量。
    ///
    /// proto3 的 `bool` 没有 presence，未设置与 `false` 无法区分，因此请求只能
    /// “补开”增量，无法关闭服务端配置已开启的增量。
    pub(crate) fn resolve(config_enabled: bool, options: Option<&CzscOptions>) -> Self {
        let request_enabled = options.is_some_and(|options| options.enable_incremental);
        if config_enabled || request_enabled {
            Self::Incremental
        } else {
            Self::Full
        }
    }
}

impl AnalysisEngine {
    pub(crate) fn new(cache_capacity: usize) -> Self {
        Self {
            store: IncrementalStore::new(cache_capacity),
        }
    }

    pub(crate) fn analyze(
        &self,
        request: &AnalyzeRequest,
        params: &AnalysisParams,
        mode: IncrementalMode,
    ) -> Result<AnalysisOutcome, AnalysisError> {
        if mode == IncrementalMode::Full {
            let snapshot = SnapshotAnalyzer::new(params).build_snapshot(request)?;
            return Ok(AnalysisOutcome {
                snapshot,
                evolution: Evolution::Full,
            });
        }
        self.analyze_incremental(request, params)
    }

    fn analyze_incremental(
        &self,
        request: &AnalyzeRequest,
        params: &AnalysisParams,
    ) -> Result<AnalysisOutcome, AnalysisError> {
        let settings = AnalysisSettings::new(request, params)?;
        let analyzer = SnapshotAnalyzer::new(params);
        let bars = analyzer.raw_bars(&settings, None)?;
        if bars.is_empty() {
            return Err(AnalysisError::invalid_request(
                "bars are empty after applying analysis window",
            ));
        }

        let key = IncrementalKey {
            freq_hint: request.freq_hint.clone(),
            max_bi_num: settings.max_bi_num(),
            min_bi_len: settings.min_bi_len(),
            analysis_window: settings.analysis_window().bars(),
        };
        let wants_signals = settings.wants_signals();
        let mut lease = self.store.lease(key);

        let mut served = None;
        if let Some(state) = lease.state_mut()
            && state.accept(&bars, wants_signals) == AcceptOutcome::Served
        {
            let points = state.signals();
            served = Some(SnapshotAssembler::new(&settings, &state.czsc, points).assemble(None)?);
        }
        if let Some(snapshot) = served {
            lease.commit();
            return Ok(AnalysisOutcome {
                snapshot,
                evolution: Evolution::Incremental,
            });
        }

        // `accept` 返回 NotApplicable 时状态未被修改：全量失败需放回旧状态，
        // 与旧实现一致（成功时由下面的 replace 覆盖）。
        let (snapshot, outcome) = match analyzer.analyze_with_settings(&settings, &bars, None) {
            Ok(result) => result,
            Err(err) => {
                lease.commit();
                return Err(err);
            }
        };
        lease.replace(IncrementalState::new(bars, outcome.czsc, outcome.evaluator));
        lease.commit();
        Ok(AnalysisOutcome {
            snapshot,
            evolution: Evolution::Full,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::{
        channel, nonzero_count, sample_request, test_config, test_params,
    };

    fn assert_snapshot_eq(a: &AnalysisSnapshot, b: &AnalysisSnapshot) {
        assert_eq!(a.channels, b.channels);
        assert_eq!(a.fractals, b.fractals);
        assert_eq!(a.strokes, b.strokes);
        assert_eq!(a.pivots, b.pivots);
        assert_eq!(a.signals, b.signals);
        assert_eq!(a.segments, b.segments);

        let am = a.meta.as_ref().expect("meta");
        let bm = b.meta.as_ref().expect("meta");
        assert_eq!(am.freq, bm.freq);
        assert_eq!(am.data_len, bm.data_len);
        assert_eq!(am.input_hash, bm.input_hash);
        assert_eq!(am.max_bi_num, bm.max_bi_num);
        assert_eq!(am.min_bi_len, bm.min_bi_len);
        assert_eq!(am.analysis_window, bm.analysis_window);
        assert_eq!(am.analyzer_version, bm.analyzer_version);
    }

    #[test]
    fn incremental_and_full_paths_match() {
        let request = sample_request(true);
        let params = test_params();
        let engine = AnalysisEngine::new(2);

        let first = engine
            .analyze(&request, &params, IncrementalMode::Incremental)
            .expect("first");
        assert_eq!(first.evolution, Evolution::Full);
        let second = engine
            .analyze(&request, &params, IncrementalMode::Incremental)
            .expect("second");
        assert_eq!(second.evolution, Evolution::Incremental);

        let full = SnapshotAnalyzer::new(&params)
            .build_snapshot(&request)
            .expect("full");
        assert_snapshot_eq(&second.snapshot, &full);
        assert_eq!(
            channel(&second.snapshot, 101).values.len(),
            request.data_len as usize
        );
        assert!(nonzero_count(&channel(&second.snapshot, 101).values) > 0);
    }

    #[test]
    fn disabled_incremental_always_returns_full() {
        let request = sample_request(false);
        let params = test_params();
        let engine = AnalysisEngine::new(2);

        for _ in 0..2 {
            let outcome = engine
                .analyze(&request, &params, IncrementalMode::Full)
                .expect("full");
            assert_eq!(outcome.evolution, Evolution::Full);
        }
    }

    #[test]
    fn incremental_mode_can_be_enabled_but_not_disabled_by_request() {
        let off = CzscOptions {
            enable_incremental: false,
            ..Default::default()
        };
        let on = CzscOptions {
            enable_incremental: true,
            ..Default::default()
        };

        // 配置关闭时，请求可以补开（proto3 bool 无 presence，只能单向加强）。
        assert_eq!(IncrementalMode::resolve(false, None), IncrementalMode::Full);
        assert_eq!(
            IncrementalMode::resolve(false, Some(&off)),
            IncrementalMode::Full
        );
        assert_eq!(
            IncrementalMode::resolve(false, Some(&on)),
            IncrementalMode::Incremental
        );
        // 配置开启时，请求无法关闭。
        assert_eq!(
            IncrementalMode::resolve(true, Some(&off)),
            IncrementalMode::Incremental
        );
    }

    #[test]
    fn meta_window_stays_on_params_side() {
        let mut request = sample_request(false);
        request.options.as_mut().expect("options").analysis_window = 4;
        let config = test_config();
        let params = AnalysisParams::from(&config);
        let snapshot = SnapshotAnalyzer::new(&params)
            .build_snapshot(&request)
            .expect("snapshot");
        assert_eq!(snapshot.meta.expect("meta").analysis_window, 64);
    }
}

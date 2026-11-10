//! 输出通道构建：func_id → 对齐 DataLen 的 float[] 值序列（纯函数）。
//!
//! `build_channel` 处理笔/段/中枢/信号通道；严格派中枢通道（305–308）由
//! `build_strict_channel` 填充，二者都由 `assemble_snapshot` 按 func_id 分派。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::fx::FX;
use czsc_core::objects::mark::Mark;
use czsc_core::objects::zs::ZS;

use super::error::AnalysisError;
use super::objects::{fx_mid_index, zs_span};
use super::segs::Segment;
use super::signals::BuySellPoint;
use super::strict_pivots::StrictPivot;
use crate::proto::analysis::{ChannelKind, ChannelSeries};

/// 构建单个输出通道：func_id → kind/名称，并填充值序列。
pub fn build_channel(
    func_id: u32,
    data_len: usize,
    fractals: &[FX],
    finished_bis: &[BI],
    pivots: &[ZS],
    segs: &[Segment],
    buy_sell_points: &[BuySellPoint],
) -> Result<ChannelSeries, AnalysisError> {
    let (kind, name) = resolve_channel_meta(func_id)?;
    let mut values = vec![0.0; data_len];

    match func_id {
        101 => fill_fractal_channel(&mut values, fractals, Mark::G),
        102 => fill_fractal_channel(&mut values, fractals, Mark::D),
        201 => fill_bi_points_channel(&mut values, finished_bis),
        202 => fill_bi_direction_channel(&mut values, finished_bis),
        203 => fill_seg_points_channel(&mut values, finished_bis, segs),
        204 => fill_seg_direction_channel(&mut values, finished_bis, segs),
        301 => fill_zs_channel(&mut values, pivots, ZsField::Upper),
        302 => fill_zs_channel(&mut values, pivots, ZsField::Lower),
        303 => fill_zs_channel(&mut values, pivots, ZsField::Middle),
        304 => fill_zs_boundary_channel(&mut values, pivots),
        401..=406 => fill_signal_channel(&mut values, buy_sell_points, func_id),
        901..=903 => {}
        _ => return Err(unsupported_func_id(func_id)),
    }

    Ok(ChannelSeries {
        func_id,
        kind,
        name: name.to_string(),
        values,
    })
}

fn unsupported_func_id(func_id: u32) -> AnalysisError {
    AnalysisError::invalid_request(format!("unsupported func_id: {func_id}"))
}

/// 未注册 func_id 的占位通道（全 0）：新旧版本混用时避免整请求失败。
pub(crate) fn zero_channel(func_id: u32, data_len: usize) -> ChannelSeries {
    ChannelSeries {
        func_id,
        kind: 0,
        name: format!("unsupported_{func_id}"),
        values: vec![0.0f32; data_len],
    }
}

/// 严格派中枢通道（305–308）+ 段级三买三卖（407/408）。
pub(crate) fn build_strict_channel(
    func_id: u32,
    data_len: usize,
    pivots: &[StrictPivot],
    signals: &[BuySellPoint],
) -> Result<ChannelSeries, AnalysisError> {
    let (kind, name) = resolve_strict_channel_meta(func_id)?;
    let mut values = vec![0.0f32; data_len];
    match func_id {
        305 => fill_strict_price(&mut values, pivots, StrictField::Upper),
        306 => fill_strict_price(&mut values, pivots, StrictField::Lower),
        307 => fill_strict_price(&mut values, pivots, StrictField::Middle),
        308 => fill_strict_boundary(&mut values, pivots),
        407 | 408 => fill_signal_channel(&mut values, signals, func_id),
        _ => return Err(unsupported_func_id(func_id)),
    }
    Ok(ChannelSeries {
        func_id,
        kind,
        name: name.to_string(),
        values,
    })
}

fn resolve_strict_channel_meta(func_id: u32) -> Result<(i32, &'static str), AnalysisError> {
    let kind = match func_id {
        305 => (ChannelKind::ChannelZsStrictUpper as i32, "zs_strict_upper"),
        306 => (ChannelKind::ChannelZsStrictLower as i32, "zs_strict_lower"),
        307 => (
            ChannelKind::ChannelZsStrictMiddle as i32,
            "zs_strict_middle",
        ),
        308 => (
            ChannelKind::ChannelZsStrictBoundary as i32,
            "zs_strict_boundary",
        ),
        407 => (ChannelKind::ChannelBuy3Strict as i32, "buy3_strict"),
        408 => (ChannelKind::ChannelSell3Strict as i32, "sell3_strict"),
        _ => return Err(unsupported_func_id(func_id)),
    };
    Ok(kind)
}

enum StrictField {
    Upper,
    Lower,
    Middle,
}

fn fill_strict_price(values: &mut [f32], pivots: &[StrictPivot], field: StrictField) {
    let last = values.len().saturating_sub(1);
    for p in pivots {
        if p.start_index > last || p.end_index > last {
            continue;
        }
        let v = match field {
            StrictField::Upper => p.upper,
            StrictField::Lower => p.lower,
            StrictField::Middle => p.middle,
        } as f32;
        // 虚中枢取负：305–307 的价格符号即 is_sure，公式层无需额外通道即可按整段着色。
        let v = if p.is_sure { v } else { -v };
        for x in &mut values[p.start_index..=p.end_index] {
            *x = v;
        }
    }
}

/// 308 边界码 `level*10+edge`：区间内 `edge=0`（只带级别），左右界 `edge=1/2`，虚取负。
///
/// 区间内也带级别，公式才能整段判级别上色；若只在边界写值，级别在区间内恒 0。
fn fill_strict_boundary(values: &mut [f32], pivots: &[StrictPivot]) {
    let last = values.len().saturating_sub(1);
    let signed = |code: u32, sure: bool| if sure { code as f32 } else { -(code as f32) };
    for p in pivots {
        if p.start_index > last {
            continue;
        }
        let end = p.end_index.min(last);
        let base = signed(p.level * 10, p.is_sure);
        for x in &mut values[p.start_index..=end] {
            *x = base;
        }
        values[p.start_index] = signed(p.level * 10 + 1, p.is_sure);
        values[end] = signed(p.level * 10 + 2, p.is_sure);
    }
}

/// 通道元数据单一映射表：func_id → (ChannelKind, 通道名)。
///
/// `handlers/version.rs` 的 supported_func_ids 与 DLL 侧约定都以这里为准，
/// 新增通道只需改这一处。
fn resolve_channel_meta(func_id: u32) -> Result<(i32, &'static str), AnalysisError> {
    let kind = match func_id {
        101 => (ChannelKind::ChannelFxTop as i32, "fx_top"),
        102 => (ChannelKind::ChannelFxBottom as i32, "fx_bottom"),
        201 => (ChannelKind::ChannelBiPoints as i32, "bi_points"),
        202 => (ChannelKind::ChannelBiDirection as i32, "bi_direction"),
        203 => (ChannelKind::ChannelSegPoints as i32, "seg_points"),
        204 => (ChannelKind::ChannelSegDirection as i32, "seg_direction"),
        301 => (ChannelKind::ChannelZsUpper as i32, "zs_upper"),
        302 => (ChannelKind::ChannelZsLower as i32, "zs_lower"),
        303 => (ChannelKind::ChannelZsMiddle as i32, "zs_middle"),
        304 => (ChannelKind::ChannelZsBoundary as i32, "zs_boundary"),
        401 => (ChannelKind::ChannelBuy1 as i32, "buy1"),
        402 => (ChannelKind::ChannelSell1 as i32, "sell1"),
        403 => (ChannelKind::ChannelBuy2 as i32, "buy2"),
        404 => (ChannelKind::ChannelSell2 as i32, "sell2"),
        405 => (ChannelKind::ChannelBuy3 as i32, "buy3"),
        406 => (ChannelKind::ChannelSell3 as i32, "sell3"),
        901 => (ChannelKind::ChannelDebugCacheHit as i32, "debug_cache_hit"),
        902 => (
            ChannelKind::ChannelDebugServiceReady as i32,
            "debug_service_ready",
        ),
        903 => (
            ChannelKind::ChannelDebugLatencyMs as i32,
            "debug_latency_ms",
        ),
        _ => return Err(unsupported_func_id(func_id)),
    };

    Ok(kind)
}

fn fill_fractal_channel(values: &mut [f32], fractals: &[FX], mark: Mark) {
    for fx in fractals.iter().filter(|fx| fx.mark == mark) {
        if let Some(index) = fx_mid_index(fx)
            && let Some(slot) = values.get_mut(index)
        {
            *slot = fx.fx as f32;
        }
    }
}

fn fill_bi_points_channel(values: &mut [f32], finished_bis: &[BI]) {
    for bi in finished_bis {
        // 每笔的起止分型都标注（此前只填终点 fx_b，第一笔起点缺失）
        if let Some(index) = fx_mid_index(&bi.fx_a)
            && let Some(slot) = values.get_mut(index)
        {
            *slot = bi.fx_a.fx as f32;
        }
        if let Some(index) = fx_mid_index(&bi.fx_b)
            && let Some(slot) = values.get_mut(index)
        {
            *slot = bi.fx_b.fx as f32;
        }
    }
}

fn fill_bi_direction_channel(values: &mut [f32], finished_bis: &[BI]) {
    for bi in finished_bis {
        let Some(start) = fx_mid_index(&bi.fx_a) else {
            continue;
        };
        let Some(end) = fx_mid_index(&bi.fx_b) else {
            continue;
        };

        let direction = match bi.direction {
            CzscDirection::Up => 1.0,
            CzscDirection::Down => -1.0,
        };

        for value in &mut values[start.min(end)..=start.max(end)] {
            *value = direction;
        }
    }
}

/// 段端点通道（203）：段首笔起点分型与末笔终点分型处输出分型价格。
///
/// **只输出已确认段**（`is_sure=true`）；尾部未完成/暂定段不显示（图表上只画
/// 特征序列分型确认的段）。段的起止都是笔端点（分型位置），方向由 101/102
/// 顶底分型通道判定：向上段起点为底分型、终点为顶分型；向下段反之。相邻段
/// 共享端点分型，重复写值幂等。
fn fill_seg_points_channel(values: &mut [f32], finished_bis: &[BI], segs: &[Segment]) {
    for seg in segs.iter().filter(|s| s.is_sure) {
        if let Some(start_bi) = finished_bis.get(seg.start_bi_idx)
            && let Some(index) = fx_mid_index(&start_bi.fx_a)
            && let Some(slot) = values.get_mut(index)
        {
            *slot = start_bi.fx_a.fx as f32;
        }
        if let Some(end_bi) = finished_bis.get(seg.end_bi_idx)
            && let Some(index) = fx_mid_index(&end_bi.fx_b)
            && let Some(slot) = values.get_mut(index)
        {
            *slot = end_bi.fx_b.fx as f32;
        }
    }
}

/// 段方向通道（204）：已确认段覆盖区间填 ±1（向上=1，向下=-1），
/// 与 202 笔向通道对称；未确认段不填。
fn fill_seg_direction_channel(values: &mut [f32], finished_bis: &[BI], segs: &[Segment]) {
    for seg in segs.iter().filter(|s| s.is_sure) {
        let Some(start_bi) = finished_bis.get(seg.start_bi_idx) else {
            continue;
        };
        let Some(end_bi) = finished_bis.get(seg.end_bi_idx) else {
            continue;
        };
        let Some(start) = fx_mid_index(&start_bi.fx_a) else {
            continue;
        };
        let Some(end) = fx_mid_index(&end_bi.fx_b) else {
            continue;
        };

        let direction = match seg.direction {
            CzscDirection::Up => 1.0,
            CzscDirection::Down => -1.0,
        };
        for value in &mut values[start.min(end)..=start.max(end)] {
            *value = direction;
        }
    }
}

enum ZsField {
    Upper,
    Lower,
    Middle,
}

fn fill_zs_channel(values: &mut [f32], pivots: &[ZS], field: ZsField) {
    for zs in pivots {
        let Some((start, end)) = zs_span(zs) else {
            continue;
        };
        // 与 fill_zs_boundary_channel / fill_strict_price 一致：越界即跳过。
        if end >= values.len() {
            continue;
        }

        let pivot_value = match field {
            ZsField::Upper => zs.zg,
            ZsField::Lower => zs.zd,
            ZsField::Middle => zs.zz,
        } as f32;

        for value in &mut values[start..=end] {
            *value = pivot_value;
        }
    }
}

/// 中枢边界标记：每个中枢绘制区间首 K 线输出 1（左界）、尾 K 线输出 2（右界），
/// 其余为 0；既是某中枢右界又是另一中枢左界的共享 K 线输出 3（左右界共用）。
/// 配合 301/302 通道，通达信侧用折线/STICKLINE 画上/下沿与左右竖线，构成闭合
/// 矩形，且相邻中枢之间互不连接。
///
/// 相邻中枢共享端点K线（前一中枢的右界 == 后一中枢的左界：原生贪心归属
/// 下，离开/连接笔被并入后一中枢作为其第一笔）。若按顺序直接写值，共享
/// K线会被后一中枢的左界 1 覆盖，前一中枢的右界 2 丢失，图上该中枢只剩
/// 左侧竖线、没有上下边框和右边框。因此先统一写所有左界，再写右界；遇到
/// 已写过左界的共享K线则升级为 3，公式层把 3 同时当作左界和右界。
fn fill_zs_boundary_channel(values: &mut [f32], pivots: &[ZS]) {
    let mut starts = Vec::with_capacity(pivots.len());
    let mut ends = Vec::with_capacity(pivots.len());
    for zs in pivots {
        let Some((start, end)) = zs_span(zs) else {
            continue;
        };
        starts.push(start);
        ends.push(end);
    }

    for &start in &starts {
        if start < values.len() {
            values[start] = 1.0;
        }
    }
    for &end in &ends {
        if end < values.len() {
            values[end] = if values[end] > 0.5 { 3.0 } else { 2.0 };
        }
    }
}

fn fill_signal_channel(values: &mut [f32], buy_sell_points: &[BuySellPoint], func_id: u32) {
    for point in buy_sell_points.iter().filter(|p| p.func_id == func_id) {
        if let Some(slot) = values.get_mut(point.index) {
            *slot = point.price as f32;
        }
    }
}

/// 全量支持的 func_id 列表（单一来源）。
///
/// `handlers/version.rs` 的 `supported_func_ids` 与 DLL 侧约定都以这里为准，
/// 新增通道需同时更新 `resolve_channel_meta` 与本列表。
pub(crate) const SUPPORTED_FUNC_IDS: &[u32] = &[
    101, 102, 201, 202, 203, 204, 301, 302, 303, 304, 305, 306, 307, 308, 401, 402, 403, 404, 405,
    406, 407, 408, 901, 902, 903,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::strict_pivots::PivotKind;

    #[test]
    fn empty_channel_has_zeros() {
        let ch = build_channel(101, 10, &[], &[], &[], &[], &[]).expect("channel");
        assert_eq!(ch.func_id, 101);
        assert!(ch.values.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn strict_channels_encode_level_and_edges() {
        let mut p = StrictPivot {
            level: 1,
            kind: PivotKind::Segment,
            is_sure: true,
            start_index: 0,
            end_index: 2,
            upper: 10.0,
            lower: 8.0,
            middle: 9.0,
            gg: 11.0,
            dd: 7.0,
            move_count: 3,
            move_start: 0,
            move_end: 0,
        };
        let boundary = build_strict_channel(308, 5, std::slice::from_ref(&p), &[]).expect("ch");
        assert_eq!(boundary.values[0], 11.0, "level1 左界 = 11");
        assert_eq!(boundary.values[1], 10.0, "区间内 = level*10");
        assert_eq!(boundary.values[2], 12.0, "level1 右界 = 12");
        let upper = build_strict_channel(305, 5, std::slice::from_ref(&p), &[]).expect("ch");
        assert_eq!(&upper.values[0..=2], &[10.0, 10.0, 10.0]);

        p.is_sure = false;
        let virt = build_strict_channel(308, 5, std::slice::from_ref(&p), &[]).expect("ch");
        assert_eq!(virt.values[0], -11.0, "虚中枢取负");
        let virt_upper = build_strict_channel(305, 5, std::slice::from_ref(&p), &[]).expect("ch");
        assert_eq!(virt_upper.values[0], -10.0, "305–307 虚中枢价格取负");
    }

    /// SUPPORTED_FUNC_IDS 里的每个 id 都必须能被某个元数据解析器识别（防漏注册）。
    #[test]
    fn supported_ids_are_all_resolvable() {
        for id in SUPPORTED_FUNC_IDS {
            assert!(
                resolve_channel_meta(*id).is_ok() || resolve_strict_channel_meta(*id).is_ok(),
                "func_id {id} 缺元数据"
            );
        }
        assert!(resolve_channel_meta(999).is_err());
    }
}

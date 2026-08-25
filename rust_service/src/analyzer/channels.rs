//! 输出通道构建：func_id → 对齐 DataLen 的 float[] 值序列（纯函数）。
//!
//! `build_channel` 是唯一入口：先解析通道元数据（kind/名称，单一映射表），
//! 再按 func_id 分派到对应填充器。分型/笔/中枢/段/信号各通道的填充器
//! 只做"对象 → 价格/方向值"的写入，不承担算法逻辑。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::fx::FX;
use czsc_core::objects::mark::Mark;
use czsc_core::objects::zs::ZS;

use super::error::AnalysisError;
use super::objects::{fx_mid_index, zs_span};
use super::segs::Segment;
use super::signals::BuySellPoint;
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
        if let Some(index) = fx_mid_index(fx) {
            values[index] = fx.fx as f32;
        }
    }
}

fn fill_bi_points_channel(values: &mut [f32], finished_bis: &[BI]) {
    for bi in finished_bis {
        // 每笔的起止分型都标注（此前只填终点 fx_b，第一笔起点缺失）
        if let Some(index) = fx_mid_index(&bi.fx_a) {
            values[index] = bi.fx_a.fx as f32;
        }
        if let Some(index) = fx_mid_index(&bi.fx_b) {
            values[index] = bi.fx_b.fx as f32;
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
        {
            values[index] = start_bi.fx_a.fx as f32;
        }
        if let Some(end_bi) = finished_bis.get(seg.end_bi_idx)
            && let Some(index) = fx_mid_index(&end_bi.fx_b)
        {
            values[index] = end_bi.fx_b.fx as f32;
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
        values[point.index] = point.price as f32;
    }
}

/// 全量支持的 func_id 列表（单一来源）。
///
/// `handlers/version.rs` 的 `supported_func_ids` 与 DLL 侧约定都以这里为准，
/// 新增通道需同时更新 `resolve_channel_meta` 与本列表。
pub(crate) const SUPPORTED_FUNC_IDS: &[u32] = &[
    101, 102, 201, 202, 301, 302, 303, 304, 401, 402, 403, 404, 405, 406, 901, 902, 903,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_meta_covers_all_supported_ids() {
        const ALL: [u32; 17] = [101, 102, 201, 202, 301, 302, 303, 304, 401, 402, 403, 404, 405, 406, 901, 902, 903];
        for id in ALL {
            assert!(resolve_channel_meta(id).is_ok(), "func_id {id} 缺元数据");
        }
        assert!(resolve_channel_meta(999).is_err());
    }

    #[test]
    fn empty_channel_has_zeros() {
        let ch = build_channel(101, 10, &[], &[], &[], &[], &[]).expect("channel");
        assert_eq!(ch.func_id, 101);
        assert!(ch.values.iter().all(|v| *v == 0.0));
    }
}
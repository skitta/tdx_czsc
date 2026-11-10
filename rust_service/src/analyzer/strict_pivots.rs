//! 严格派中枢：段中枢（level 1）→ 9 段升级 / 扩张（level 2）→ 扩张（level 3）。
//! 规则见 `docs/严格中枢规则.md`。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;

use super::objects::{bi_end_index, bi_start_index};
use super::pivot_rules::{trend_of, trim_back, trim_front};
use super::segs::Segment;
use super::signals::BuySellPoint;
use crate::proto::analysis::SignalKind;

/// 段参与成枢的最小笔数（未形成段内笔中枢时的后备门槛）。
/// 段内形成至少 1 个三笔中枢时，不受此笔数限制。
pub const MIN_SEG_BI_LEN: usize = 5;
/// 扩张合并的子中枢数上限（防止一路吞并）。
const MAX_EXPANSION_MEMBERS: usize = 4;
/// 扩张合并后允许的最大 K 线跨度。
const MAX_EXPANSION_SPAN_BARS: usize = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PivotKind {
    Segment,
    NineSegmentUpgrade,
    Expansion,
}

impl PivotKind {
    pub fn as_i32(self) -> i32 {
        match self {
            PivotKind::Segment => 1,
            PivotKind::NineSegmentUpgrade => 2,
            PivotKind::Expansion => 3,
        }
    }
}

/// 段（严格中枢的构成单元）。
#[derive(Debug, Clone, Copy)]
pub struct Move {
    pub seg_index: usize,
    pub high: f64,
    pub low: f64,
    pub start_price: f64,
    pub end_price: f64,
    pub start_index: usize,
    pub end_index: usize,
    pub is_sure: bool,
    pub up: bool,
    /// 是否满足成枢门槛（笔数足够且有可对齐的 K 线下标）。
    pub qualified: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StrictPivot {
    pub level: u32,
    pub kind: PivotKind,
    pub is_sure: bool,
    pub start_index: usize,
    pub end_index: usize,
    pub upper: f64,
    pub lower: f64,
    pub middle: f64,
    pub gg: f64,
    pub dd: f64,
    pub move_count: usize,
    /// 构成段在 `segments` 中的起止下标（闭区间）。
    pub move_start: usize,
    pub move_end: usize,
}

#[derive(Debug, Clone, Default)]
pub struct StrictPivotSet {
    pub pivots: Vec<StrictPivot>,
    pub segments: Vec<Move>,
}

/// 段包络：段内全部构成笔的 `get_high()` 最大 / `get_low()` 最小。
pub fn seg_envelope(bis: &[BI], seg: &Segment) -> Option<(f64, f64)> {
    let slice = bis.get(seg.start_bi_idx..=seg.end_bi_idx)?;
    let mut high = f64::NEG_INFINITY;
    let mut low = f64::INFINITY;
    for bi in slice {
        high = high.max(bi.get_high());
        low = low.min(bi.get_low());
    }
    (high.is_finite() && low.is_finite()).then_some((high, low))
}

fn to_move(bis: &[BI], seg: &Segment, seg_index: usize) -> Move {
    let slice = bis.get(seg.start_bi_idx..=seg.end_bi_idx);
    let envelope = seg_envelope(bis, seg);
    let bi_count = slice.map_or(0, |s| s.len());
    // 门槛二选一：段内含笔中枢，或段笔数达到 MIN_SEG_BI_LEN。
    let has_pen_pivot = bi_count < MIN_SEG_BI_LEN
        && slice.is_some_and(|s| {
            czsc_core::analyze::utils::get_zs_seq(s)
                .iter()
                .any(|zs| zs.bis.len() >= 3 && zs.zd <= zs.zg)
        });
    let (high, low) = envelope.unwrap_or((0.0, 0.0));
    let start_index = bi_start_index(bis.get(seg.start_bi_idx));
    let end_index = bi_end_index(bis.get(seg.end_bi_idx));
    Move {
        seg_index,
        high,
        low,
        start_price: slice.and_then(|s| s.first()).map_or(0.0, |b| b.fx_a.fx),
        end_price: slice.and_then(|s| s.last()).map_or(0.0, |b| b.fx_b.fx),
        // 对不齐时归 0 只作区间占位：qualified=false 的段不参与成枢，
        // 因此不会画出跨全图的中枢。
        start_index: start_index.unwrap_or(0),
        end_index: end_index.unwrap_or(0),
        is_sure: seg.is_sure,
        up: seg.direction == CzscDirection::Up,
        qualified: start_index.is_some()
            && end_index.is_some()
            && envelope.is_some()
            && (has_pen_pivot || bi_count >= MIN_SEG_BI_LEN),
    }
}

/// 构建全部级别的严格中枢（纯函数）。
pub fn build_strict_pivots(bis: &[BI], segs: &[Segment]) -> StrictPivotSet {
    let moves: Vec<Move> = segs
        .iter()
        .enumerate()
        .map(|(i, seg)| to_move(bis, seg, i))
        .collect();
    let level1 = segment_pivots(&moves);
    let (upgraded, upgraded_ids) = nine_segment_pivots(&moves, &level1);
    let expansion_members: Vec<StrictPivot> = level1
        .iter()
        .enumerate()
        .filter(|(i, _)| !upgraded_ids.contains(i))
        .map(|(_, p)| *p)
        .collect();
    let level2_expansion = expand_level(&expansion_members, 2);

    let mut pivots = level1.clone();
    pivots.extend(upgraded.iter().copied());
    pivots.extend(level2_expansion.iter().copied());
    let mut level2: Vec<StrictPivot> = upgraded
        .iter()
        .chain(level2_expansion.iter())
        .copied()
        .collect();
    // `expand_level` 依赖时间顺序；upgraded 与 expansion 两段各自有序，
    // 拼接后必须重新排序，否则级别 3 会跨过中间中枢合并。
    level2.sort_by_key(|p| (p.start_index, p.end_index));
    pivots.extend(expand_level(&level2, 3));

    StrictPivotSet {
        pivots,
        segments: moves,
    }
}

/// 级别 1：3 段（已过门槛）重叠，后续有交集则延伸；
/// 再按技术方案 §8.2 做前端/后端方向与相邻间隔优化。
fn segment_pivots(moves: &[Move]) -> Vec<StrictPivot> {
    let Some(window_start_price) = moves.first().map(|m| m.start_price) else {
        return Vec::new();
    };
    let n = moves.len();
    let mut fronts: Vec<(StrictPivot, CzscDirection)> = Vec::new();
    let mut prev_middle: Option<f64> = None;
    let mut prev_trend: Option<CzscDirection> = None;
    let mut i = 0;
    while i + 2 < n {
        let (a, b, c) = (&moves[i], &moves[i + 1], &moves[i + 2]);
        if !(a.qualified && b.qualified && c.qualified) {
            i += 1;
            continue;
        }
        let upper = a.high.min(b.high).min(c.high);
        let lower = a.low.max(b.low).max(c.low);
        if upper < lower {
            i += 1;
            continue;
        }
        let middle = (upper + lower) / 2.0;
        let trend = trend_of(middle, prev_middle.zip(prev_trend), window_start_price);
        let trend_up = trend == CzscDirection::Up;

        let mut members = vec![i, i + 1, i + 2];
        let mut j = i + 3;
        while j < n {
            let m = &moves[j];
            // 完全离开区间即结束；未过门槛的段若仍在区间内则跳过。
            if m.high < lower || m.low > upper {
                break;
            }
            if !m.qualified {
                j += 1;
                continue;
            }
            members.push(j);
            j += 1;
        }

        // 前端：首段与趋势同向视为进入/连接段，剔除后区间由剩余前 3 段重算。
        // 与笔中枢一致，不重新扩展成员，只在原划分上裁剪两端。
        if !trim_front(&mut members, trend_up, |k| moves[*k].up) {
            i += 1;
            continue;
        }
        let Some(pivot) = make_segment_pivot(moves, &members) else {
            i += 1;
            continue;
        };
        prev_middle = Some(pivot.middle);
        prev_trend = Some(trend);
        fronts.push((pivot, trend));
        // 下一轮从离开段之后扫描（离开段不属于任何中枢）。
        i = j + 1;
    }

    // 后端：从右往左处理，后一中枢已确定才能判断相邻间隔。
    let mut result = Vec::new();
    for (pivot, trend) in fronts.into_iter().rev() {
        let next_start = result.last().map(|p: &StrictPivot| p.start_index);
        let Some(trimmed) = trim_segment_back(moves, pivot, trend, next_start) else {
            continue;
        };
        result.push(trimmed);
    }
    result.reverse();
    result
}

/// 用构成段下标重建级别 1 中枢：区间取前 3 段重叠，GG/DD 取全部成员。
fn make_segment_pivot(moves: &[Move], members: &[usize]) -> Option<StrictPivot> {
    if members.len() < 3 {
        return None;
    }
    let upper = members[..3]
        .iter()
        .map(|&k| moves[k].high)
        .fold(f64::INFINITY, f64::min);
    let lower = members[..3]
        .iter()
        .map(|&k| moves[k].low)
        .fold(f64::NEG_INFINITY, f64::max);
    if upper < lower {
        return None;
    }
    let first = moves[*members.first()?];
    let last = moves[*members.last()?];
    Some(StrictPivot {
        level: 1,
        kind: PivotKind::Segment,
        is_sure: members.iter().all(|&k| moves[k].is_sure),
        start_index: first.start_index,
        end_index: last.end_index,
        upper,
        lower,
        middle: (upper + lower) / 2.0,
        gg: members
            .iter()
            .map(|&k| moves[k].high)
            .fold(f64::NEG_INFINITY, f64::max),
        dd: members
            .iter()
            .map(|&k| moves[k].low)
            .fold(f64::INFINITY, f64::min),
        move_count: members.len(),
        move_start: *members.first()?,
        move_end: *members.last()?,
    })
}

/// 后端：末段与趋势同向（离开段）或与后一中枢共享端点时回退，
/// 最少保留 3 段；仍不满足则整组剔除。规则见 `pivot_rules::trim_back`。
fn trim_segment_back(
    moves: &[Move],
    pivot: StrictPivot,
    trend: CzscDirection,
    next_start: Option<usize>,
) -> Option<StrictPivot> {
    let trend_up = trend == CzscDirection::Up;
    let mut members: Vec<usize> = (pivot.move_start..=pivot.move_end)
        .filter(|&k| moves[k].qualified)
        .collect();
    if !trim_back(&mut members, trend_up, next_start, |k| {
        (moves[*k].up, Some(moves[*k].end_index))
    }) {
        return None;
    }
    make_segment_pivot(moves, &members)
}

/// 3 段组的自身区间（`GZ >= GD` 才合法）。
fn group_range(moves: &[Move], start: usize) -> Option<(f64, f64)> {
    let a = moves.get(start)?;
    let b = moves.get(start + 1)?;
    let c = moves.get(start + 2)?;
    if !(a.qualified && b.qualified && c.qualified) {
        return None;
    }
    let gz = a.high.min(b.high).min(c.high);
    let gd = a.low.max(b.low).max(c.low);
    (gz >= gd).then_some((gz, gd))
}

/// 级别 2：9 段升级（3 个 3 段组；`move_count >= 9` 才尝试）。
///
/// shortcut: 按物理连续段分组；`move_start..=move_end` 内若有跳过的未过门槛段，
/// 升级会提前停止。当前真实数据每中枢段数远小于 9，暂不保存构成段下标列表。
fn nine_segment_pivots(moves: &[Move], level1: &[StrictPivot]) -> (Vec<StrictPivot>, Vec<usize>) {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for (idx, p) in level1.iter().enumerate() {
        if p.move_count < 9 {
            continue;
        }
        let start = p.move_start;
        let mut zg = 0.0;
        let mut zd = 0.0;
        let mut ok = start + 8 <= p.move_end;
        for g in 0..3 {
            let Some((gz, gd)) = group_range(moves, start + g * 3) else {
                ok = false;
                break;
            };
            if g == 0 {
                zg = gz;
                zd = gd;
            } else {
                zg = zg.min(gz);
                zd = zd.max(gd);
            }
            if zg < zd {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let mut end = start + 8;
        let mut count = 3;
        let mut g = start + 9;
        while g + 2 <= p.move_end {
            let Some((gz, gd)) = group_range(moves, g) else {
                break;
            };
            if gz < zd || gd > zg {
                break;
            }
            zg = zg.min(gz);
            zd = zd.max(gd);
            if zg < zd {
                break;
            }
            end = g + 2;
            count += 1;
            g += 3;
        }
        let slice = &moves[start..=end];
        out.push(StrictPivot {
            level: 2,
            kind: PivotKind::NineSegmentUpgrade,
            is_sure: p.is_sure && slice.iter().all(|m| m.is_sure),
            start_index: p.start_index,
            end_index: moves[end].end_index,
            upper: zg,
            lower: zd,
            middle: (zg + zd) / 2.0,
            gg: slice
                .iter()
                .map(|m| m.high)
                .fold(f64::NEG_INFINITY, f64::max),
            dd: slice.iter().map(|m| m.low).fold(f64::INFINITY, f64::min),
            move_count: count,
            move_start: start,
            move_end: end,
        });
        ids.push(idx);
    }
    (out, ids)
}

/// 同级别中枢扩张：GG/DD 与 ZG/ZD 双条件 + 链长/跨度上限。
fn expand_level(members: &[StrictPivot], level: u32) -> Vec<StrictPivot> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < members.len() {
        let mut cur = members[i];
        let mut members_count = 1usize; // 已合并的子中枢数
        let mut j = i + 1;
        while j < members.len() {
            let next = members[j];
            if next.start_index <= cur.end_index {
                break;
            }
            let gg_overlap = cur.gg >= next.dd && next.gg >= cur.dd;
            let cores_overlap = cur.upper >= next.lower && next.upper >= cur.lower;
            if !gg_overlap || !cores_overlap {
                break;
            }
            if members_count + 1 > MAX_EXPANSION_MEMBERS
                || next.end_index.saturating_sub(cur.start_index) > MAX_EXPANSION_SPAN_BARS
            {
                break;
            }
            let upper = cur.gg.min(next.gg);
            let lower = cur.dd.max(next.dd);
            if upper < lower {
                break;
            }
            cur.upper = upper;
            cur.lower = lower;
            cur.middle = (upper + lower) / 2.0;
            cur.gg = cur.gg.max(next.gg);
            cur.dd = cur.dd.min(next.dd);
            cur.end_index = next.end_index;
            cur.is_sure &= next.is_sure;
            cur.level = level;
            cur.kind = PivotKind::Expansion;
            members_count += 1;
            cur.move_count = members_count;
            j += 1;
        }
        if members_count >= 2 {
            out.push(cur);
        }
        i = j;
    }
    out
}

/// 每根 bar 至多一个可见中枢：实优先、级别高优先、move_count 大优先、开始靠后优先。
pub fn select_visible(pivots: &[StrictPivot], data_len: usize) -> Vec<StrictPivot> {
    let mut order: Vec<usize> = (0..pivots.len()).collect();
    order.sort_by(|&a, &b| priority(&pivots[b]).cmp(&priority(&pivots[a])));
    let mut occupied = vec![false; data_len.max(1)];
    let mut visible = Vec::new();
    for idx in order {
        let p = &pivots[idx];
        if p.end_index >= occupied.len() || p.start_index > p.end_index {
            continue;
        }
        if occupied[p.start_index..=p.end_index].iter().any(|x| *x) {
            continue;
        }
        for x in &mut occupied[p.start_index..=p.end_index] {
            *x = true;
        }
        visible.push(*p);
    }
    visible.sort_by_key(|p| p.start_index);
    visible
}

fn priority(p: &StrictPivot) -> (u8, u32, u32, u32) {
    (
        p.is_sure as u8,
        p.level,
        p.move_count as u32,
        p.start_index as u32,
    )
}

/// 段级三买/三卖（407/408）：只对已确认的级别 1 中枢。
pub fn build_strict_signals(set: &StrictPivotSet) -> Vec<BuySellPoint> {
    let mut points = Vec::new();
    for p in set
        .pivots
        .iter()
        .filter(|p| p.kind == PivotKind::Segment && p.level == 1 && p.is_sure)
    {
        // move_end 是最后一个并入的段；其后可能还有被跳过的未过门槛段，
        // 因此从段列表里找第一个完全离开区间的段作为离开段。
        let Some(leave_idx) = (p.move_end + 1..set.segments.len()).find(|&i| {
            let m = &set.segments[i];
            m.high < p.lower || m.low > p.upper
        }) else {
            continue;
        };
        let leave = &set.segments[leave_idx];
        let Some(confirm) = set.segments.get(leave_idx + 1) else {
            continue;
        };
        if !leave.is_sure || !confirm.is_sure {
            continue;
        }
        // 离开段之后任一段重新进入中枢区间，本次信号失败。
        if set
            .segments
            .iter()
            .skip(leave_idx + 2)
            .any(|m| m.high >= p.lower && m.low <= p.upper)
        {
            continue;
        }
        if !leave.up && leave.low > p.upper && confirm.up && confirm.low > p.upper {
            points.push(BuySellPoint {
                func_id: 407,
                kind: SignalKind::SignalSegBuy3,
                index: leave.end_index,
                price: leave.end_price,
                label: "段级三买".to_string(),
            });
        } else if leave.up && leave.high < p.lower && !confirm.up && confirm.high < p.lower {
            points.push(BuySellPoint {
                func_id: 408,
                kind: SignalKind::SignalSegSell3,
                index: leave.end_index,
                price: leave.end_price,
                label: "段级三卖".to_string(),
            });
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::make_series_bi;

    /// 用价格拐点构造连续笔：笔 k = pts[k] -> pts[k+1]。
    fn bis_from(pts: &[f64]) -> Vec<BI> {
        pts.windows(2)
            .enumerate()
            .map(|(i, w)| make_series_bi(i as i64, w[0], w[1]))
            .collect()
    }

    fn seg(start: usize, end: usize, is_sure: bool) -> Segment {
        seg_with_dir(start, end, CzscDirection::Up, is_sure)
    }

    fn seg_with_dir(start: usize, end: usize, dir: CzscDirection, is_sure: bool) -> Segment {
        Segment {
            start_bi_idx: start,
            end_bi_idx: end,
            direction: dir,
            is_sure,
        }
    }

    #[test]
    fn three_qualified_segments_form_pivot_with_fixed_range() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, 17.0, 13.0, 17.0, 14.0, 16.0, 15.0, 12.0, 16.0,
            13.0, 15.0,
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg_with_dir(0, 4, CzscDirection::Up, true),
            seg_with_dir(5, 9, CzscDirection::Down, true),
            seg_with_dir(10, 14, CzscDirection::Up, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("level1 pivot");
        assert_eq!((p.upper, p.lower), (16.0, 13.0));
        assert_eq!(p.move_count, 3);
        assert!(p.is_sure);
        assert_eq!(p.kind, PivotKind::Segment);
    }

    #[test]
    fn unqualified_short_segments_do_not_form_pivot() {
        let pts = [10.0, 20.0, 11.0, 19.0, 12.0, 18.0, 13.0];
        let bis = bis_from(&pts);
        let segs = [seg(0, 1, true), seg(2, 3, true), seg(4, 5, true)];
        assert!(build_strict_pivots(&bis, &segs).pivots.is_empty());
    }

    #[test]
    fn leaving_segment_ends_pivot_and_is_excluded() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg0
            17.0, 13.0, 17.0, 14.0, 16.0, // seg1
            15.0, 12.0, 16.0, 13.0, 15.0, // seg2 → 中枢 [13,16]
            20.0, 17.0, 19.0, 18.0, 18.0, // seg3 跨界延伸
            18.0, 16.0, 17.0, 15.0, 16.0, 18.0, // seg4 仍在区间
            20.0, 17.0, 19.0, 18.0, 20.0, // seg5 整体在外（离开）
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg_with_dir(0, 4, CzscDirection::Up, true),
            seg_with_dir(5, 9, CzscDirection::Down, true),
            seg_with_dir(10, 14, CzscDirection::Up, true),
            seg_with_dir(15, 19, CzscDirection::Down, true),
            seg_with_dir(20, 24, CzscDirection::Up, true),
            seg_with_dir(25, 29, CzscDirection::Down, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("level1 pivot");
        assert_eq!(p.move_count, 5, "跨界段并入、整体在外段不得并入");
        assert_eq!(p.move_end, 4);
        assert_eq!(p.end_index, set.segments[4].end_index);
    }

    #[test]
    fn virtual_pivot_when_any_segment_unsure() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, 17.0, 13.0, 17.0, 14.0, 16.0, 15.0, 12.0, 16.0,
            13.0, 15.0,
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg_with_dir(0, 4, CzscDirection::Up, false),
            seg_with_dir(5, 9, CzscDirection::Down, true),
            seg_with_dir(10, 14, CzscDirection::Up, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let p = set.pivots.first().expect("pivot");
        assert!(!p.is_sure);
    }

    /// 9 段全部落在同一区间 → 升级为 level 2（3 个 3 段组）。
    #[test]
    fn nine_segments_upgrade_to_level2() {
        let mut pts = vec![10.0, 20.0];
        for _ in 0..44 {
            pts.push(if pts.len() % 2 == 0 { 20.0 } else { 10.0 });
        }
        let bis = bis_from(&pts);
        let segs: Vec<Segment> = (0..9)
            .map(|k| {
                let dir = if k % 2 == 0 {
                    CzscDirection::Down
                } else {
                    CzscDirection::Up
                };
                seg_with_dir(k * 5, k * 5 + 4, dir, true)
            })
            .collect();
        let set = build_strict_pivots(&bis, &segs);
        let up = set
            .pivots
            .iter()
            .find(|p| p.kind == PivotKind::NineSegmentUpgrade)
            .expect("9 段升级");
        assert_eq!(up.level, 2);
        assert_eq!(up.move_count, 3);
        assert_eq!((up.upper, up.lower), (20.0, 10.0));
    }

    /// 扩张：GG/DD 重叠但核心区间不重叠 → 不合并；双条件满足 → level 2。
    #[test]
    fn expansion_requires_core_overlap() {
        let mk = |upper: f64, lower: f64, gg: f64, dd: f64, start: usize, end: usize| StrictPivot {
            level: 1,
            kind: PivotKind::Segment,
            is_sure: true,
            start_index: start,
            end_index: end,
            upper,
            lower,
            middle: (upper + lower) / 2.0,
            gg,
            dd,
            move_count: 3,
            move_start: 0,
            move_end: 2,
        };
        let far = [
            mk(20.0, 18.0, 100.0, 5.0, 0, 10),
            mk(90.0, 88.0, 95.0, 85.0, 12, 20),
        ];
        assert!(expand_level(&far, 2).is_empty(), "核心区不重叠不得扩张");

        let near = [
            mk(20.0, 18.0, 22.0, 15.0, 0, 10),
            mk(19.0, 15.0, 21.0, 14.0, 12, 20),
        ];
        let merged = expand_level(&near, 2);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].kind, PivotKind::Expansion);
        assert_eq!(merged[0].level, 2);
        assert_eq!(merged[0].move_count, 2);
    }

    /// 可见性：实中枢优先于高级别虚中枢。
    #[test]
    fn visible_prefers_sure_pivot() {
        let mk = |level: u32, sure: bool, start: usize, end: usize| StrictPivot {
            level,
            kind: PivotKind::Segment,
            is_sure: sure,
            start_index: start,
            end_index: end,
            upper: 10.0,
            lower: 8.0,
            middle: 9.0,
            gg: 11.0,
            dd: 7.0,
            move_count: 3,
            move_start: 0,
            move_end: 2,
        };
        let pivots = [mk(1, true, 0, 5), mk(2, false, 2, 8)];
        let visible = select_visible(&pivots, 20);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].level, 1, "已确认的低级别优先于虚高级别");
    }

    /// 三买锚在离开段终点，而非段包络最低点或随后上涨段顶部。
    #[test]
    fn third_buy_anchors_at_leaving_segment_end() {
        let pts = [
            14.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg1
            17.0, 13.0, 17.0, 14.0, 16.0, // seg2
            15.0, 12.0, 16.0, 13.0, 15.0, // seg3 → 中枢 [13,16]
            20.0, 17.0, 19.0, 18.0, 18.0, // seg4 上破（延伸）
            17.0, 19.0, 17.2, 18.0, 17.5, // seg5 下跌离开（low 17，终点 17.5）
            19.0, 18.0, 20.0, 19.0, 21.0, // seg6 向上确认
        ];
        let bis = bis_from(&pts);
        let mut segs = vec![
            seg_with_dir(0, 4, CzscDirection::Down, true),
            seg_with_dir(5, 9, CzscDirection::Up, true),
            seg_with_dir(10, 14, CzscDirection::Down, true),
            seg_with_dir(15, 19, CzscDirection::Up, true),
        ];
        segs.push(Segment {
            start_bi_idx: 20,
            end_bi_idx: 24,
            direction: CzscDirection::Down,
            is_sure: true,
        });
        segs.push(seg(25, 29, true));
        let set = build_strict_pivots(&bis, &segs);
        let points = build_strict_signals(&set);
        let buy = points.iter().find(|p| p.func_id == 407).expect("三买");
        assert_eq!(buy.index, set.segments[4].end_index);
        assert!(
            (buy.price - 17.5).abs() < 1e-9,
            "必须用离开段终点价，不能用段包络最低点"
        );
    }

    /// 段内 3 笔但有笔中枢时，也应满足“完成次级别走势”门槛。
    #[test]
    fn pen_pivot_qualifies_short_segment() {
        let pts = [
            10.0, 20.0, 12.0, 18.0, // seg1: 3 笔且含笔中枢
            11.0, 21.0, 13.0, // seg2
            19.0, 12.0, 22.0, // seg3
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg_with_dir(0, 2, CzscDirection::Down, true),
            seg_with_dir(3, 5, CzscDirection::Up, true),
            seg_with_dir(6, 8, CzscDirection::Down, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("level1 pivot");
        assert_eq!(p.move_count, 3);
    }

    /// 延伸遇到未过门槛的段应跳过，后面的合资格段仍可延伸。
    #[test]
    fn extension_skips_unqualified_segment() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg1
            17.0, 13.0, 17.0, 14.0, 16.0, // seg2
            15.0, 12.0, 16.0, 13.0, 15.0, // seg3 → 中枢 [13,16]
            15.0, 16.0, 17.0, 18.0, // seg4: 3 笔、无笔中枢，区间内
            18.0, 16.0, 15.0, 16.0, 14.0, 17.0, // seg5: 合资格、仍与中枢相交
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg_with_dir(0, 4, CzscDirection::Up, true),
            seg_with_dir(5, 9, CzscDirection::Down, true),
            seg_with_dir(10, 14, CzscDirection::Up, true),
            seg_with_dir(15, 17, CzscDirection::Down, true),
            seg_with_dir(18, 22, CzscDirection::Up, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("level1 pivot");
        assert_eq!(p.move_end, 4, "未过门槛的 seg4 不并入，seg5 必须延伸");
        assert_eq!(p.move_count, 4);
    }

    /// 离开段无需满足成枢门槛；407 仍应产出。
    #[test]
    fn third_buy_allows_unqualified_leaving_segment() {
        let pts = [
            14.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg1
            17.0, 13.0, 17.0, 14.0, 16.0, // seg2
            15.0, 12.0, 16.0, 13.0, 15.0, // seg3 → 中枢 [13,16]
            20.0, 17.0, 19.0, 18.0, 18.0, // seg4 上破（延伸）
            17.0, 19.0, 20.0, // seg5: 3 笔离开段，无笔中枢
            19.0, 21.0, 18.0, 20.0, 22.0, // seg6 向上确认
        ];
        let bis = bis_from(&pts);
        let mut segs = vec![
            seg_with_dir(0, 4, CzscDirection::Down, true),
            seg_with_dir(5, 9, CzscDirection::Up, true),
            seg_with_dir(10, 14, CzscDirection::Down, true),
            seg_with_dir(15, 19, CzscDirection::Up, true),
        ];
        segs.push(Segment {
            start_bi_idx: 20,
            end_bi_idx: 22,
            direction: CzscDirection::Down,
            is_sure: true,
        });
        segs.push(seg(23, 27, true));
        let set = build_strict_pivots(&bis, &segs);
        assert!(!set.segments[4].qualified, "测试前提：离开段未过门槛");
        let buy = build_strict_signals(&set)
            .into_iter()
            .find(|p| p.func_id == 407)
            .expect("三买");
        assert!((buy.price - 20.0).abs() < 1e-9);
    }

    /// 确认段之后再回到中枢区间，信号必须失效。
    #[test]
    fn third_buy_invalidated_by_later_reentry() {
        let pts = [
            14.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg1
            17.0, 13.0, 17.0, 14.0, 16.0, // seg2
            15.0, 12.0, 16.0, 13.0, 15.0, // seg3 → 中枢 [13,16]
            20.0, 17.0, 19.0, 18.0, 18.0, // seg4 上破（延伸）
            17.0, 19.0, 17.2, 18.0, 17.5, // seg5 下跌离开
            19.0, 18.0, 20.0, 19.0, 21.0, // seg6 向上确认
            18.0, 15.0, 16.0, 14.0, 15.0, // seg7 重新进入 [13,16]
        ];
        let bis = bis_from(&pts);
        let mut segs = vec![
            seg_with_dir(0, 4, CzscDirection::Down, true),
            seg_with_dir(5, 9, CzscDirection::Up, true),
            seg_with_dir(10, 14, CzscDirection::Down, true),
            seg_with_dir(15, 19, CzscDirection::Up, true),
        ];
        segs.push(Segment {
            start_bi_idx: 20,
            end_bi_idx: 24,
            direction: CzscDirection::Down,
            is_sure: true,
        });
        segs.push(seg(25, 29, true));
        segs.push(Segment {
            start_bi_idx: 30,
            end_bi_idx: 34,
            direction: CzscDirection::Down,
            is_sure: true,
        });
        let set = build_strict_pivots(&bis, &segs);
        assert!(
            build_strict_signals(&set).iter().all(|p| p.func_id != 407),
            "后续段重新进入中枢后不得保留 407"
        );
    }

    /// 扩张只合并时间上不重叠的相邻中枢。
    #[test]
    fn expansion_rejects_overlapping_pivots() {
        let mk = |start: usize, end: usize| StrictPivot {
            level: 1,
            kind: PivotKind::Segment,
            is_sure: true,
            start_index: start,
            end_index: end,
            upper: 20.0,
            lower: 18.0,
            middle: 19.0,
            gg: 22.0,
            dd: 15.0,
            move_count: 3,
            move_start: 0,
            move_end: 2,
        };
        assert!(expand_level(&[mk(0, 10), mk(10, 20)], 2).is_empty());
    }

    /// 9 段升级成级别 2；后续每 3 段成组继续并入，13 段时升级 12 段、move_count=4。
    #[test]
    fn nine_segments_upgrade_and_extend_by_groups() {
        let mut pts = Vec::new();
        for _ in 0..13 {
            pts.extend([13.0, 16.0, 13.0, 16.0, 13.0, 16.0]);
        }
        let bis = bis_from(&pts);
        let segs: Vec<_> = (0..13)
            .map(|k| {
                let dir = if k % 2 == 0 {
                    CzscDirection::Down
                } else {
                    CzscDirection::Up
                };
                seg_with_dir(k * 5, k * 5 + 4, dir, true)
            })
            .collect();
        let set = build_strict_pivots(&bis, &segs);

        let level1 = set
            .pivots
            .iter()
            .find(|p| p.kind == PivotKind::Segment)
            .expect("level1");
        assert_eq!(level1.move_count, 13);

        let upgraded = set
            .pivots
            .iter()
            .find(|p| p.kind == PivotKind::NineSegmentUpgrade)
            .expect("level2 upgrade");
        assert_eq!(upgraded.level, 2);
        assert_eq!(upgraded.move_count, 4, "9 段 + 1 个三段组");
        assert!((upgraded.upper - 16.0).abs() < 1e-9);
        assert!((upgraded.lower - 13.0).abs() < 1e-9);
        assert_eq!(upgraded.move_end, 11);
    }

    /// 前三组收缩失败时不得产出级别 2（调用方保留原级别 1）。
    #[test]
    fn failed_nine_segment_upgrade_returns_nothing() {
        let mk = |start: usize, high: f64, low: f64| Move {
            seg_index: start,
            high,
            low,
            start_price: low,
            end_price: high,
            start_index: start * 10,
            end_index: start * 10 + 9,
            is_sure: true,
            up: true,
            qualified: true,
        };
        // 三组区间互不相交：G1[10,12] / G2[20,22] / G3[30,32]。
        let mut moves = Vec::new();
        for &(high, low) in &[(12.0, 10.0), (22.0, 20.0), (32.0, 30.0)] {
            for _ in 0..3 {
                moves.push(mk(moves.len(), high, low));
            }
        }
        let level1 = StrictPivot {
            level: 1,
            kind: PivotKind::Segment,
            is_sure: true,
            start_index: 0,
            end_index: 89,
            upper: 12.0,
            lower: 10.0,
            middle: 11.0,
            gg: 32.0,
            dd: 10.0,
            move_count: 9,
            move_start: 0,
            move_end: 8,
        };
        let (out, ids) = nine_segment_pivots(&moves, &[level1]);
        assert!(out.is_empty());
        assert!(ids.is_empty());
    }

    /// 扩张链长上限 4、时间跨度上限 250 根。
    #[test]
    fn expansion_respects_member_and_span_limits() {
        let mk = |start: usize, end: usize| StrictPivot {
            level: 2,
            kind: PivotKind::NineSegmentUpgrade,
            is_sure: true,
            start_index: start,
            end_index: end,
            upper: 20.0,
            lower: 18.0,
            middle: 19.0,
            gg: 22.0,
            dd: 15.0,
            move_count: 3,
            move_start: 0,
            move_end: 8,
        };

        let chain = [mk(0, 10), mk(11, 20), mk(21, 30), mk(31, 40), mk(41, 50)];
        let out = expand_level(&chain, 3);
        assert_eq!(out.len(), 1, "第 5 个中枢不应单独产出");
        assert_eq!(out[0].move_count, 4, "链长上限 4");
        assert_eq!(out[0].level, 3);

        let span = [mk(0, 10), mk(100, 110), mk(200, 210), mk(300, 310)];
        let out = expand_level(&span, 3);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].move_count, 3, "合并到第 4 个会超过 250 根");
        assert_eq!(out[0].end_index, 210);
    }

    /// ZG == ZD 的零宽度中枢合法。
    #[test]
    fn zero_width_pivot_is_allowed() {
        let pts = vec![10.0; 16];
        let bis = bis_from(&pts);
        let segs = [seg(0, 4, true), seg(5, 9, true), seg(10, 14, true)];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("zero width pivot");
        assert!((p.upper - 10.0).abs() < 1e-9);
        assert!((p.lower - 10.0).abs() < 1e-9);
        assert!((p.middle - 10.0).abs() < 1e-9);
    }

    fn mv(k: usize, high: f64, low: f64, up: bool) -> Move {
        Move {
            seg_index: k,
            high,
            low,
            start_price: low,
            end_price: high,
            start_index: k * 10,
            end_index: k * 10 + 10,
            is_sure: true,
            up,
            qualified: true,
        }
    }

    /// 前端：上升行情首段向上（进入段）被剔除，并从下一段重新扫描；
    /// 区间由剩余前 3 段重算。
    #[test]
    fn segment_front_trims_entry_and_rescans() {
        let moves = [
            mv(0, 20.0, 10.0, true),  // 进入段
            mv(1, 19.0, 12.0, false), // 新首段
            mv(2, 18.0, 13.0, true),
            mv(3, 18.0, 13.0, false),
            mv(4, 25.0, 20.0, true), // 离开段
        ];
        let pivots = segment_pivots(&moves);
        assert_eq!(pivots.len(), 1);
        let p = pivots[0];
        assert_eq!(p.move_start, 1, "进入段不得属于中枢");
        assert_eq!(p.move_end, 3);
        assert_eq!(p.move_count, 3);
        assert_eq!((p.upper, p.lower), (18.0, 13.0));
        assert_eq!(p.start_index, 10);
        assert_eq!(p.end_index, 40);
    }

    /// 后端：上升行情末段向上（离开段）被剔除。
    #[test]
    fn segment_back_trims_leaving_segment() {
        let moves = [
            mv(0, 19.0, 12.0, false),
            mv(1, 18.0, 13.0, true),
            mv(2, 18.0, 13.0, false),
            mv(3, 18.0, 13.0, true),  // 延伸进来的离开段
            mv(4, 25.0, 20.0, false), // 整体在外
        ];
        let pivots = segment_pivots(&moves);
        assert_eq!(pivots.len(), 1);
        let p = pivots[0];
        assert_eq!(p.move_end, 2, "末段与趋势同向应剔除");
        assert_eq!(p.move_count, 3);
        assert_eq!(p.end_index, 30);
    }

    /// 相邻中枢至少间隔一个离开段；不足 3 段又共享端点时整组剔除。
    #[test]
    fn segment_adjacent_pivots_keep_gap() {
        let moves = [
            mv(0, 19.0, 12.0, false),
            mv(1, 18.0, 13.0, true),
            mv(2, 18.0, 13.0, false),
            mv(3, 25.0, 20.0, true), // 前中枢离开段
            mv(4, 22.0, 15.0, false),
            mv(5, 21.0, 16.0, true),
            mv(6, 21.0, 16.0, false),
        ];
        let pivots = segment_pivots(&moves);
        assert_eq!(pivots.len(), 2);
        assert_eq!(pivots[0].end_index, 30);
        assert_eq!(pivots[1].start_index, 40);
        assert!(
            pivots[0].end_index < pivots[1].start_index,
            "相邻中枢不得共享端点"
        );
        assert!(
            trim_segment_back(
                &moves,
                pivots[0],
                CzscDirection::Up,
                Some(pivots[0].end_index)
            )
            .is_none(),
            "3 段仍共享端点时应整组剔除"
        );
    }

    /// 三卖锚在向下离开段终点，且确认段必须向下。
    #[test]
    fn third_sell_anchors_at_leaving_segment_end() {
        let pts = [
            16.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg0
            17.0, 13.0, 17.0, 14.0, 16.0, // seg1
            15.0, 12.0, 16.0, 13.0, 15.0, // seg2 → 中枢 [13,16]
            10.0, 14.0, 11.0, 13.0, 10.0, // seg3 下破（延伸）
            12.0, 11.0, 12.5, 11.5, 12.0, // seg4 上涨离开（high 12.5 < 13）
            9.0, 11.0, 10.0, 9.5, 9.0, // seg5 向下确认
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg_with_dir(0, 4, CzscDirection::Up, true),
            seg_with_dir(5, 9, CzscDirection::Down, true),
            seg_with_dir(10, 14, CzscDirection::Up, true),
            seg_with_dir(15, 19, CzscDirection::Down, true),
            seg_with_dir(20, 24, CzscDirection::Up, true),
            seg_with_dir(25, 29, CzscDirection::Down, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let sell = build_strict_signals(&set)
            .into_iter()
            .find(|p| p.func_id == 408)
            .expect("三卖");
        assert_eq!(sell.index, set.segments[4].end_index);
        assert!((sell.price - 12.0).abs() < 1e-9);
        assert!(
            build_strict_signals(&set).iter().all(|p| p.func_id != 407),
            "向下离开不得产出三买"
        );
    }
}

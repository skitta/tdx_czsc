//! 笔中枢构建：原生 czsc `get_zs_seq` + 前端/后端优化（纯笔序列操作）。
//!
//! 原生 `get_zs_seq` 的笔归属是贪心的：与当前中枢区间无交集的"离开/连接笔"
//! 并入下一个中枢作为其第一笔，导致中枢首笔/末笔可能混入进入/连接笔。这里
//! 保留原生的中枢划分，优化两端：
//!
//! **前端（第一笔）**：中枢第一笔必须与行情趋势相反——上升行情中第一笔
//! 向下（"下上下"重叠）、下跌行情中第一笔向上（"上下上"重叠）。行情方向
//! 按中枢高低位置判定：后一个中枢中轴（zz）比前一个高 → 上升行情，更低 →
//! 下降行情；中轴平齐沿用前一趋势。第一个中枢无前序中枢，以其位置与窗口
//! 起点价格比较兜底。首笔与趋势同向视为进入/连接笔，予以剔除；区间
//! [zd, zg] 由剔除后剩余前 3 笔的重叠重新计算。
//!
//! **后端（最后一笔）**：中枢最后一笔必须与行情趋势相反（与前端对称，
//! 中枢为奇数笔结构）——末笔与趋势同向视为离开笔，予以剔除。
//!
//! **相邻间隔**：前一个中枢后端与后一中枢前端不得共享端点，必须至少间隔
//! 一笔——后端剔除后，离开/连接笔落在两个中枢矩形之间，不属于任一中枢。
//!
//! 剔除后不足 3 笔、前 3 笔无重叠、或无法同时满足上述规则的候选中枢整组
//! 剔除（不绘制）。

use czsc_core::analyze::utils::get_zs_seq;
use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::zs::ZS;

use super::objects::{bi_end_index, bi_start_index};
use super::signals::BuySellPoint;

/// 在原生 czsc 中枢（`get_zs_seq`）之上优化前端与后端。
pub fn build_pivots(bis: &[BI]) -> Vec<ZS> {
    let native = get_zs_seq(bis);

    // 第一遍：前端优化 + 行情趋势链（方向按中枢高低位置递推）
    let mut fronts: Vec<(ZS, CzscDirection)> = Vec::new();
    let mut prev_zz: Option<f64> = None;
    let mut prev_trend: Option<CzscDirection> = None;
    for zs in &native {
        // 行情方向：按中枢高低位置判定（当前中枢用原生中轴与前一中枢的
        // 最终中轴比较；剔除进入笔对中轴影响很小，不影响方向判定）
        let trend = match prev_zz {
            None => {
                if zs.zz > bis[0].fx_a.fx {
                    CzscDirection::Up
                } else {
                    CzscDirection::Down
                }
            }
            Some(prev) if zs.zz > prev => CzscDirection::Up,
            Some(prev) if zs.zz < prev => CzscDirection::Down,
            Some(_) => prev_trend.unwrap_or(CzscDirection::Up),
        };
        let Some(trimmed) = trim_pivot_front(zs, trend) else {
            continue;
        };
        prev_zz = Some(trimmed.zz);
        prev_trend = Some(trend);
        fronts.push((trimmed, trend));
    }

    // 第二遍：后端优化（从后往前处理——需要后一保留中枢的起点判断相邻间隔，
    // 倒序时 result 的最后一个即"后一中枢"，被剔除的中枢自动跳过）
    let mut result: Vec<ZS> = Vec::new();
    for (zs, trend) in fronts.into_iter().rev() {
        let next_start = result.last().and_then(|p| bi_start_index(p.bis.first()));
        let Some(trimmed) = trim_pivot_back(zs, trend, next_start) else {
            continue;
        };
        result.push(trimmed);
    }
    result.reverse();
    result
}

/// 剔除中枢的进入/连接笔（前端），返回优化后的中枢。
///
/// 前端：首笔与趋势同向（= 进入/连接笔）时剔除；剔除后不足 3 笔不成中枢。
/// 区间 [zd, zg] 由剔除后的前 3 笔重叠重新计算，前 3 笔无重叠则不成中枢。
fn trim_pivot_front(zs: &ZS, trend: CzscDirection) -> Option<ZS> {
    let mut bis: Vec<BI> = zs.bis.clone();
    if bis.len() < 3 {
        return None;
    }
    // 前端：首笔与趋势同向 = 进入/连接笔，剔除
    if bis[0].direction == trend {
        bis.remove(0);
        if bis.len() < 3 {
            return None;
        }
    }
    // 区间 [zd, zg] = 前 3 笔的重叠（剔除进入笔后重新计算）
    let zd = bis[..3]
        .iter()
        .map(|b| b.get_low())
        .fold(f64::NEG_INFINITY, f64::max);
    let zg = bis[..3]
        .iter()
        .map(|b| b.get_high())
        .fold(f64::INFINITY, f64::min);
    if zg < zd {
        return None;
    }
    Some(ZS::new(bis))
}

/// 剔除中枢的离开笔（后端），返回优化后的中枢。
///
/// 规则一（末笔方向）：中枢最后一笔必须与行情趋势相反——上升行情最后一笔
/// 向下、下跌行情最后一笔向上（与前端对称，中枢为奇数笔结构）；末笔与
/// 趋势同向（离开笔）时剔除。
///
/// 规则二（相邻间隔）：前一个中枢后端与后一中枢前端至少间隔一笔，不得
/// 共享端点。
///
/// 优先级：**前端优化优先于后端**。两端发生端点共享时，后中枢前端（首笔
/// 方向规则）不可改动，由前中枢末端退回一笔（间隔规则）；退回后若末笔
/// 方向与趋势同向，还需再退一笔（方向规则）——循环直至两条规则都满足，
/// 或缩至最少 3 笔。缩到最少 3 笔仍无法同时满足两条规则的候选中枢整组
/// 剔除（不绘制）。
fn trim_pivot_back(zs: ZS, trend: CzscDirection, next_start: Option<usize>) -> Option<ZS> {
    let mut bis = zs.bis;
    loop {
        if bis.len() <= 3 {
            break;
        }
        let last = bis.last().unwrap();
        // 规则二（间隔，优先级低于前端）：与后一中枢共享/相交端点时，
        // 后中枢前端不动（前端优先），本中枢（前中枢）末端退回一笔
        let shares = next_start.is_some_and(|s| bi_end_index(bis.last()).is_some_and(|e| e >= s));
        // 规则一（方向）：退回后若末笔与趋势同向（离开笔），还需再退一笔
        let wrong_dir = last.direction == trend;
        if !shares && !wrong_dir {
            break;
        }
        bis.pop();
    }
    if bis.len() < 3 {
        return None;
    }
    let last = bis.last().unwrap();
    if last.direction == trend
        || next_start.is_some_and(|s| bi_end_index(bis.last()).is_some_and(|e| e >= s))
    {
        return None;
    }
    Some(ZS::new(bis))
}

/// 三买/三卖缠论严格化：**依附中枢，每个中枢最多一个三买（405）和一个三卖（406）**。
///
/// 三买/三卖按"锚点之前最近结束的中枢"归属（三买/三卖是中枢被突破/跌破后的
/// 回抽确认点，出现在该中枢结束之后）；同一中枢的第二个同向点丢弃；锚点前
/// 无任何已结束中枢的点（无法归属）也丢弃。
pub(crate) fn strictize_third_bs_by_zs(points: &mut Vec<BuySellPoint>, pivots: &[ZS]) {
    let mut seen_405: Vec<usize> = Vec::new();
    let mut seen_406: Vec<usize> = Vec::new();
    points.retain(|p| {
        if p.func_id != 405 && p.func_id != 406 {
            return true;
        }
        // 归属到锚点之前最近结束的中枢（用中枢组最后一笔终点）
        let Some(zi) = pivots
            .iter()
            .rposition(|zs| bi_end_index(zs.bis.last()).is_some_and(|e| e < p.index))
        else {
            return false;
        };
        if p.func_id == 405 {
            if seen_405.contains(&zi) {
                return false;
            }
            seen_405.push(zi);
        } else {
            if seen_406.contains(&zi) {
                return false;
            }
            seen_406.push(zi);
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::make_series_bi;
    use crate::analyzer::objects::{bi_end_index, bi_start_index};

    /// 中枢前端/后端优化（原生 get_zs_seq + build_pivots 全链路）：
    /// - 前端：上升行情中枢第一笔向下、下跌行情第一笔向上；
    /// - 后端：上升行情中枢最后一笔向下、下跌行情最后一笔向上（奇数笔
    ///   结构）；末笔与趋势同向（离开笔）被剔除；
    /// - 相邻间隔：前一个中枢后端与后一中枢前端至少间隔一笔（离开/连接
    ///   笔不属于任一中枢）；无法满足时整组剔除。
    ///
    /// 场景 1/2：上涨/下跌各两个中枢，均为 3 笔 [下,上,下]/[上,下,上]，
    /// 离开笔落在两中枢之间。场景 3：中枢向下破位（末笔跌破下沿）与后一
    /// 中枢共享端点，无法同时满足规则，整组剔除。场景 4：端点共享时前端
    /// 优先——前中枢先退一笔（间隔）、再退一笔（方向）。
    #[test]
    fn pivot_edges_follow_trend() {
        // 场景 1：上涨——两个中枢依次抬高，均为 [下,上,下]
        let up_pts = [
            (10.0, 20.0), // 0 上：窗口起点进入段（剔除）
            (20.0, 12.0), // 1 下
            (12.0, 19.0), // 2 上
            (19.0, 13.0), // 3 下
            (13.0, 25.0), // 4 上：离开笔（不属于任一中枢）
            (25.0, 22.0), // 5 下
            (22.0, 29.0), // 6 上
            (29.0, 23.0), // 7 下
            (23.0, 31.0), // 8 上：离开笔（不属于任一中枢）
            (31.0, 26.0), // 9 下：不成中枢
        ];
        let bis: Vec<BI> = up_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = build_pivots(&bis);
        assert_eq!(pivots.len(), 2, "上涨场景应保留两个中枢");
        for (k, zs) in pivots.iter().enumerate() {
            assert_eq!(zs.bis.len(), 3, "中枢{k}应为奇数笔 3 笔结构");
            assert_eq!(
                zs.sdir,
                CzscDirection::Down,
                "上涨行情中枢{k}第一笔必须向下"
            );
            assert_eq!(
                zs.bis.last().unwrap().direction,
                CzscDirection::Down,
                "上涨行情中枢{k}最后一笔必须向下"
            );
            if k > 0 {
                assert!(zs.zz > pivots[k - 1].zz, "后中枢必须高于前中枢");
            }
        }
        assert_eq!(pivots[0].bis[0].fx_a.fx, 20.0, "中枢 0 应从下跌笔 1 开始");
        assert_eq!(pivots[1].bis[0].fx_a.fx, 25.0, "中枢 1 应从下跌笔 5 开始");
        // 离开笔（13→25、23→31）不属于任一中枢
        for zs in &pivots {
            for b in &zs.bis {
                assert!(
                    b.fx_a.fx != 13.0 && b.fx_a.fx != 23.0,
                    "离开笔不得属于任一中枢"
                );
            }
        }
        // 相邻间隔：前中枢末笔终点下标 < 后中枢首笔起点下标（至少间隔一笔）
        assert!(
            bi_end_index(pivots[0].bis.last()).unwrap()
                < bi_start_index(pivots[1].bis.first()).unwrap(),
            "前中枢后端必须与后一中枢前端至少间隔一笔"
        );

        // 场景 2：下跌——两个中枢依次走低，均为 [上,下,上]
        let down_pts = [
            (20.0, 10.0), // 0 下：窗口起点进入段（剔除）
            (10.0, 18.0), // 1 上
            (18.0, 11.0), // 2 下
            (11.0, 17.0), // 3 上
            (17.0, 8.0),  // 4 下：离开笔（不属于任一中枢）
            (8.0, 10.0),  // 5 上
            (10.0, 6.0),  // 6 下
            (6.0, 9.0),   // 7 上
            (9.0, 5.0),   // 8 下：离开笔（不属于任一中枢）
            (5.0, 7.0),   // 9 上：不成中枢
        ];
        let bis: Vec<BI> = down_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = build_pivots(&bis);
        assert_eq!(pivots.len(), 2, "下跌场景应保留两个中枢");
        for (k, zs) in pivots.iter().enumerate() {
            assert_eq!(zs.bis.len(), 3, "中枢{k}应为奇数笔 3 笔结构");
            assert_eq!(zs.sdir, CzscDirection::Up, "下跌行情中枢{k}第一笔必须向上");
            assert_eq!(
                zs.bis.last().unwrap().direction,
                CzscDirection::Up,
                "下跌行情中枢{k}最后一笔必须向上"
            );
            if k > 0 {
                assert!(zs.zz < pivots[k - 1].zz, "后中枢必须低于前中枢");
            }
        }
        assert_eq!(pivots[0].bis[0].fx_a.fx, 10.0, "中枢 0 应从上涨笔 1 开始");
        assert_eq!(pivots[1].bis[0].fx_a.fx, 8.0, "中枢 1 应从上涨笔 5 开始");
        assert!(
            bi_end_index(pivots[0].bis.last()).unwrap()
                < bi_start_index(pivots[1].bis.first()).unwrap(),
            "前中枢后端必须与后一中枢前端至少间隔一笔"
        );

        // 场景 3：中枢 [12,19] 向下破位（末笔 19→8 跌破下沿），后一中枢起点
        // 与该末笔终点共享端点——3 笔无法再缩，整组剔除，仅保留后一中枢。
        let reversal_pts = [
            (10.0, 20.0), // 0 上：进入段（剔除）
            (20.0, 12.0), // 1 下
            (12.0, 19.0), // 2 上
            (19.0, 8.0),  // 3 下：破位，末笔终点与笔 4 起点共享
            (8.0, 10.0),  // 4 上：后一中枢首笔
            (10.0, 6.0),  // 5 下
            (6.0, 9.0),   // 6 上
            (9.0, 5.0),   // 7 下
            (5.0, 8.0),   // 8 上
        ];
        let bis: Vec<BI> = reversal_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = build_pivots(&bis);
        assert_eq!(pivots.len(), 1, "破位中枢无法满足间隔规则应整组剔除");
        assert_eq!(
            pivots[0].sdir,
            CzscDirection::Up,
            "下跌行情（后中枢更低）第一笔向上"
        );
        assert_eq!(pivots[0].bis[0].fx_a.fx, 8.0, "保留的中枢应从笔 4 开始");
        assert_eq!(pivots[0].bis.len(), 5, "后一中枢保留 5 笔（末笔向上）");

        // 场景 4：端点共享时前端优先——后中枢前端不动，前中枢末端先退一笔
        // （间隔），退回后末笔与趋势同向再退一笔（方向），剩 [下,上,下]。
        // 前中枢 [13,19] 末两笔为 U(13→25) 上破 + D(25→11) 深跌回踩（与
        // 后中枢首笔 U(11→11.5) 共享端点 11），先后退 D(25→11)（间隔）、
        // 再退 U(13→25)（方向）。
        let sweep_pts = [
            (10.0, 20.0), // 0 上：进入段（剔除）
            (20.0, 12.0), // 1 下
            (12.0, 19.0), // 2 上
            (19.0, 13.0), // 3 下
            (13.0, 25.0), // 4 上：上破
            (25.0, 11.0), // 5 下：深跌回踩，与笔 6 共享端点 11
            (11.0, 11.5), // 6 上：完全低于原生区间下沿（12）→ 新中枢首笔
            (11.5, 9.0),  // 7 下
            (9.0, 12.0),  // 8 上
            (12.0, 8.0),  // 9 下
            (8.0, 11.0),  // 10 上
        ];
        let bis: Vec<BI> = sweep_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = build_pivots(&bis);
        assert_eq!(pivots.len(), 2, "深跌回踩场景应保留两个中枢");
        // 前中枢：先退间隔一笔（笔 5），再退方向一笔（笔 4），剩 [下,上,下]
        assert_eq!(
            pivots[0].sdir,
            CzscDirection::Down,
            "前中枢（上升行情）第一笔向下"
        );
        assert_eq!(pivots[0].bis.len(), 3, "前中枢应退回两笔后剩 3 笔");
        assert_eq!(pivots[0].bis[0].fx_a.fx, 20.0, "前中枢应从下跌笔 1 开始");
        assert_eq!(
            pivots[0].bis.last().unwrap().fx_b.fx,
            13.0,
            "前中枢末笔应为 D(19→13)，不再包含笔 4/5"
        );
        // 后中枢：前端优先未被触碰（首笔 U 保留），趋势向下、末笔向上
        assert_eq!(
            pivots[1].sdir,
            CzscDirection::Up,
            "后中枢（下跌行情）第一笔向上"
        );
        assert_eq!(pivots[1].bis[0].fx_a.fx, 11.0, "后中枢首笔应为 U(11→11.5)");
        assert_eq!(pivots[1].bis.len(), 5, "后中枢保留 5 笔");
        assert_eq!(
            pivots[1].bis.last().unwrap().direction,
            CzscDirection::Up,
            "下跌行情后中枢末笔向上"
        );
        // 间隔：前中枢末笔终点（笔 3 终点）在笔 4/5 之后，与后中枢起点隔开
        assert!(
            bi_end_index(pivots[0].bis.last()).unwrap()
                < bi_start_index(pivots[1].bis.first()).unwrap(),
            "前中枢后端必须与后一中枢前端至少间隔一笔"
        );
    }
}

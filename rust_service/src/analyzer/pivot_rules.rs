//! 笔中枢与段中枢共用的边缘规则（技术方案 §8.2 / `docs/严格中枢规则.md` §2）。
//!
//! 两个级别只裁剪原生划分、不重新扩展成员；规则改动只改本模块。

use czsc_core::objects::direction::Direction as CzscDirection;

/// 行情趋势：首个中枢按中轴与窗口起点价比较（高则上升，否则下降）；
/// 后续中枢与前一保留中枢的中轴比较，平齐沿用前一趋势。
/// `prev` 是前一保留中枢的 `(中轴, 趋势)`。
pub(super) fn trend_of(
    middle: f64,
    prev: Option<(f64, CzscDirection)>,
    window_start_price: f64,
) -> CzscDirection {
    match prev {
        None if middle > window_start_price => CzscDirection::Up,
        None => CzscDirection::Down,
        Some((prev_middle, _)) if middle > prev_middle => CzscDirection::Up,
        Some((prev_middle, _)) if middle < prev_middle => CzscDirection::Down,
        Some((_, prev_trend)) => prev_trend,
    }
}

/// 前端：首单元与趋势同向视为进入/连接单元并剔除。
/// 返回剔除后是否仍够 3 个构成单元。
pub(super) fn trim_front<T>(
    members: &mut Vec<T>,
    trend_up: bool,
    is_up: impl Fn(&T) -> bool,
) -> bool {
    if members.len() < 3 {
        return false;
    }
    if is_up(&members[0]) == trend_up {
        members.remove(0);
    }
    members.len() >= 3
}

/// 后端：末单元与趋势同向（离开单元）或与后一中枢共享端点时回退，
/// 最少保留 3 个；仍不满足返回 false（整组剔除）。
/// `view` 返回 `(是否向上, 结束下标)`；前端优先，故只回退后端。
pub(super) fn trim_back<T>(
    members: &mut Vec<T>,
    trend_up: bool,
    next_start: Option<usize>,
    view: impl Fn(&T) -> (bool, Option<usize>),
) -> bool {
    let ok = |last: &T| {
        let (up, end) = view(last);
        let shares = next_start.is_some_and(|s| end.is_some_and(|e| e >= s));
        up != trend_up && !shares
    };
    while members.len() > 3 {
        let Some(last) = members.last() else {
            break;
        };
        if ok(last) {
            break;
        }
        members.pop();
    }
    if members.len() < 3 {
        return false;
    }
    members.last().is_some_and(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trend_first_pivot_uses_window_start_price() {
        assert_eq!(trend_of(12.0, None, 10.0), CzscDirection::Up);
        assert_eq!(trend_of(9.0, None, 10.0), CzscDirection::Down);
        assert_eq!(
            trend_of(10.0, None, 10.0),
            CzscDirection::Down,
            "平齐按下降"
        );
    }

    #[test]
    fn trend_carries_previous_on_tie() {
        let prev = Some((10.0, CzscDirection::Up));
        assert_eq!(trend_of(11.0, prev, 0.0), CzscDirection::Up);
        assert_eq!(trend_of(9.0, prev, 0.0), CzscDirection::Down);
        assert_eq!(
            trend_of(10.0, prev, 0.0),
            CzscDirection::Up,
            "平齐沿用前趋势"
        );
    }

    #[test]
    fn trim_front_drops_leader_and_requires_three() {
        let mut members = vec![true, false, true, false];
        assert!(trim_front(&mut members, true, |x| *x));
        assert_eq!(members, vec![false, true, false]);

        let mut kept = vec![false, true, false];
        assert!(trim_front(&mut kept, true, |x| *x));
        assert_eq!(kept, vec![false, true, false]);

        let mut too_short = vec![true, false, true];
        assert!(!trim_front(&mut too_short, true, |x| *x));
        assert_eq!(too_short, vec![false, true]);
    }

    #[test]
    fn trim_back_drops_leaving_and_shared_tail() {
        // 末单元与趋势同向（离开单元）→ 回退。
        let mut members = vec![(false, 0usize), (true, 1), (false, 2), (true, 3)];
        assert!(trim_back(&mut members, true, None, |x| (x.0, Some(x.1))));
        assert_eq!(members.last().unwrap().0, false);

        // 与后一中枢共享端点 → 回退。
        let mut members = vec![(false, 0usize), (true, 1), (false, 2), (false, 3)];
        assert!(trim_back(&mut members, true, Some(3), |x| (x.0, Some(x.1))));
        assert_eq!(members.len(), 3);
        assert_eq!(members.last().unwrap().1, 2);

        // 剩 3 个仍共享端点 → 整组剔除。
        let mut members = vec![(false, 0usize), (true, 1), (false, 3)];
        assert!(!trim_back(&mut members, true, Some(3), |x| (
            x.0,
            Some(x.1)
        )));

        // 剩 3 个且干净 → 保留。
        let mut members = vec![(false, 0usize), (true, 1), (false, 2)];
        assert!(trim_back(&mut members, true, Some(9), |x| (x.0, Some(x.1))));
    }
}

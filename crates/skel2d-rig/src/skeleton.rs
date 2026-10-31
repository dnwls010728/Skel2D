//! Fully automatic chains for simple elongated shapes (tails, tentacles,
//! ropes): thin the mask to its medial axis, take the longest path and split
//! it into evenly spaced joints.

use std::collections::VecDeque;

use crate::mask::{Mask, neighbors8};

/// Where the chain's root joint should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSide {
    Top,
    Bottom,
    Left,
    Right,
}

/// `segments + 1` joints in pixel coordinates from the root end to the tip,
/// or `None` if the mask has no usable medial axis.
pub fn chain_joints(mask: &Mask, segments: usize, root: RootSide) -> Option<Vec<[f32; 2]>> {
    // Thinning is O(pixels) per pass; work on a bounded copy.
    let (small, scale) = mask.downscaled(256);
    let thin = zhang_suen(&small);
    let path = longest_path(&thin)?;
    if path.len() < 2 {
        return None;
    }
    let mut pts: Vec<[f32; 2]> =
        path.iter().map(|&(x, y)| [(x as f32 + 0.5) * scale, (y as f32 + 0.5) * scale]).collect();
    let first_is_root = match root {
        RootSide::Top => pts[0][1] <= pts[pts.len() - 1][1],
        RootSide::Bottom => pts[0][1] >= pts[pts.len() - 1][1],
        RootSide::Left => pts[0][0] <= pts[pts.len() - 1][0],
        RootSide::Right => pts[0][0] >= pts[pts.len() - 1][0],
    };
    if !first_is_root {
        pts.reverse();
    }
    Some(resample(&pts, segments.max(1)))
}

/// Evenly spaced points by arc length, including both ends.
fn resample(pts: &[[f32; 2]], segments: usize) -> Vec<[f32; 2]> {
    let mut acc = vec![0.0f32];
    for w in pts.windows(2) {
        acc.push(acc.last().copied().unwrap_or_default() + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
    }
    let total = *acc.last().unwrap_or(&0.0);
    (0..=segments)
        .map(|k| {
            let d = total * k as f32 / segments as f32;
            let i = acc.partition_point(|&a| a < d).clamp(1, pts.len() - 1);
            let span = (acc[i] - acc[i - 1]).max(1e-6);
            let t = ((d - acc[i - 1]) / span).clamp(0.0, 1.0);
            [pts[i - 1][0] + (pts[i][0] - pts[i - 1][0]) * t, pts[i - 1][1] + (pts[i][1] - pts[i - 1][1]) * t]
        })
        .collect()
}

/// Zhang-Suen thinning to a one-pixel-wide skeleton.
pub fn zhang_suen(mask: &Mask) -> Mask {
    let (w, h) = (mask.width, mask.height);
    let mut px: Vec<bool> = (0..w * h).map(|i| mask.get(i % w, i / w)).collect();
    let at = |px: &[bool], x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && px[y as usize * w + x as usize];
    loop {
        let mut changed = false;
        for step in 0..2 {
            let mut remove = Vec::new();
            for y in 0..h as i64 {
                for x in 0..w as i64 {
                    if !at(&px, x, y) {
                        continue;
                    }
                    // Neighbours P2..P9 clockwise from north.
                    let n = [
                        at(&px, x, y - 1),
                        at(&px, x + 1, y - 1),
                        at(&px, x + 1, y),
                        at(&px, x + 1, y + 1),
                        at(&px, x, y + 1),
                        at(&px, x - 1, y + 1),
                        at(&px, x - 1, y),
                        at(&px, x - 1, y - 1),
                    ];
                    let b = n.iter().filter(|&&v| v).count();
                    let a = (0..8).filter(|&i| !n[i] && n[(i + 1) % 8]).count();
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let cond = if step == 0 { !(p2 && p4 && p6) && !(p4 && p6 && p8) } else { !(p2 && p4 && p8) && !(p2 && p6 && p8) };
                    if (2..=6).contains(&b) && a == 1 && cond {
                        remove.push(y as usize * w + x as usize);
                    }
                }
            }
            changed |= !remove.is_empty();
            for i in remove {
                px[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
    Mask::from_fn(w, h, |x, y| px[y * w + x])
}

/// Longest shortest-path through the largest connected piece of the
/// skeleton (two BFS passes: exact for trees, close for skeleton graphs).
fn longest_path(thin: &Mask) -> Option<Vec<(usize, usize)>> {
    let (w, h) = (thin.width, thin.height);
    let mut visited = vec![false; w * h];
    let mut best: Option<((usize, usize), usize)> = None;
    for i in 0..w * h {
        let p = (i % w, i / w);
        if !thin.get(p.0, p.1) || visited[i] {
            continue;
        }
        let (_, _, seen) = bfs(thin, p);
        let count = seen.iter().filter(|&&s| s).count();
        visited.iter_mut().zip(&seen).for_each(|(v, &s)| *v |= s);
        if best.is_none_or(|(_, c)| count > c) {
            best = Some((p, count));
        }
    }
    let (seed, _) = best?;
    let (a, _, _) = bfs(thin, seed);
    let (b, prev, _) = bfs(thin, a);
    let mut path = vec![b];
    let mut cur = b;
    while cur != a {
        cur = prev[cur.1 * w + cur.0]?;
        path.push(cur);
    }
    path.reverse();
    Some(path)
}

type Prev = Vec<Option<(usize, usize)>>;

/// Farthest pixel reached, predecessor links and the visited set.
fn bfs(m: &Mask, start: (usize, usize)) -> ((usize, usize), Prev, Vec<bool>) {
    let (w, h) = (m.width, m.height);
    let mut prev: Prev = vec![None; w * h];
    let mut seen = vec![false; w * h];
    let mut queue = VecDeque::from([start]);
    seen[start.1 * w + start.0] = true;
    let mut last = start;
    while let Some(p) = queue.pop_front() {
        last = p;
        for (nx, ny) in neighbors8(p.0, p.1, w, h) {
            let j = ny * w + nx;
            if m.get(nx, ny) && !seen[j] {
                seen[j] = true;
                prev[j] = Some(p);
                queue.push_back((nx, ny));
            }
        }
    }
    (last, prev, seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_bar_gives_horizontal_chain() {
        let mask = Mask::from_fn(200, 40, |x, y| (10..190).contains(&x) && (12..28).contains(&y));
        let joints = chain_joints(&mask, 4, RootSide::Left).unwrap();
        assert_eq!(joints.len(), 5);
        assert!(joints[0][0] < joints[4][0]);
        for j in &joints {
            assert!((j[1] - 20.0).abs() < 4.0, "{j:?}");
        }
        assert!(joints[4][0] - joints[0][0] > 140.0);
    }
}

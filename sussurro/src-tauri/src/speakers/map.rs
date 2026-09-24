//! Voice map (#144): a 2-D picture of a document's stored speaker
//! embeddings, "to see at a glance who sounds like whom". Pure: the
//! command reads `segments.json` and hands the lines here.
//!
//! The projection is plain PCA on the L2-normalised embeddings (the same
//! space the clustering works in, see [`super::cluster`]): centre them,
//! build the `d × d` covariance, and take its two leading eigenvectors by
//! subspace (block power) iteration plus a 2 × 2 Rayleigh–Ritz step. No
//! linear-algebra crate: the covariance of 256-d WeSpeaker vectors is
//! 256 × 256, which the iteration handles in milliseconds, and building it is `O(n·d²)` — well under a
//! second for 5,000 lines in a release build.
//!
//! Deterministic by construction: a fixed start vector, a fixed iteration
//! budget, and a sign convention (each axis points so that its largest
//! loading is positive, as scikit-learn's `svd_flip`), so the same
//! document always draws the same map, whatever order its lines come in.
//!
//! Distances on the map are approximate: two dimensions keep only part of
//! the differences between 256-d voice prints ([`VoiceMap::explained`]
//! says how much), so the UI says "closeness is approximate".

use crate::archive::SegmentsFile;
use serde::Serialize;

/// Most points drawn: beyond this the map is downsampled ([`downsample`]).
/// The projection still uses every line.
pub const MAX_POINTS: usize = 5_000;
/// Subspace-iteration budget. Converges far sooner on real data (the
/// leading eigenvalues of voice prints are well apart); the cap only
/// bounds the time on degenerate input.
const MAX_ITERATIONS: usize = 1_000;
/// Stop when the plane of the two axes moves less than this per iteration.
const TOLERANCE: f64 = 1e-10;

/// One line on the map.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VoiceMapPoint {
    pub segment_id: u32,
    /// The line's speaker when the map was computed (the UI prefers the
    /// item's current one, so a moved line recolours without a refetch).
    pub speaker_id: Option<String>,
    pub x: f32,
    pub y: f32,
}

/// What the Voice map card draws.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct VoiceMap {
    /// At most [`MAX_POINTS`], in the document's line order.
    pub points: Vec<VoiceMapPoint>,
    /// Lines with a usable embedding (≥ `points.len()`: more means the map
    /// was downsampled for drawing).
    pub total: usize,
    /// Share of the embeddings' variance the two axes keep, 0–1.
    pub explained: f32,
}

/// A 2-D PCA projection: one `[x, y]` per input row.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub coords: Vec<[f64; 2]>,
    /// `(λ1 + λ2) / trace`, 0 when the input has no spread.
    pub explained: f64,
}

/// Project `rows` (all the same length) onto their two principal axes,
/// after L2-normalising each row. Zero rows stay at the centre's
/// projection. Fewer than two rows, or rows without spread, give zeros.
pub fn project(rows: &[&[f32]]) -> Projection {
    let n = rows.len();
    let d = rows.first().map_or(0, |r| r.len());
    if n == 0 || d == 0 || rows.iter().any(|r| r.len() != d) {
        return Projection {
            coords: vec![[0.0; 2]; n],
            explained: 0.0,
        };
    }
    // L2-normalised rows in f64, then centred.
    let mut x: Vec<Vec<f64>> = rows
        .iter()
        .map(|r| {
            let v: Vec<f64> = r.iter().map(|&a| a as f64).collect();
            let norm = v.iter().map(|a| a * a).sum::<f64>().sqrt();
            if norm > 1e-12 {
                v.into_iter().map(|a| a / norm).collect()
            } else {
                v
            }
        })
        .collect();
    let mut mean = vec![0.0; d];
    for r in &x {
        mean.iter_mut().zip(r).for_each(|(m, a)| *m += a);
    }
    mean.iter_mut().for_each(|m| *m /= n as f64);
    for r in &mut x {
        r.iter_mut().zip(&mean).for_each(|(a, m)| *a -= m);
    }
    // Covariance (upper triangle, mirrored).
    let mut cov = vec![vec![0.0; d]; d];
    for r in &x {
        for (i, (row, &ri)) in cov.iter_mut().zip(r).enumerate() {
            if ri == 0.0 {
                continue;
            }
            row[i..]
                .iter_mut()
                .zip(&r[i..])
                .for_each(|(c, &rj)| *c += ri * rj);
        }
    }
    // Scale, then mirror the upper triangle down.
    cov.iter_mut().flatten().for_each(|c| *c /= n as f64);
    for i in 1..d {
        let (above, below) = cov.split_at_mut(i);
        for (j, upper) in above.iter().enumerate() {
            below[0][j] = upper[i];
        }
    }
    let trace: f64 = cov.iter().enumerate().map(|(i, row)| row[i]).sum();
    if trace <= 1e-15 {
        return Projection {
            coords: vec![[0.0; 2]; n],
            explained: 0.0,
        };
    }
    let ([v1, v2], [l1, l2]) = top_two_axes(&cov);
    let coords = x.iter().map(|r| [dot(r, &v1), dot(r, &v2)]).collect();
    Projection {
        coords,
        explained: ((l1 + l2) / trace).clamp(0.0, 1.0),
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

/// Deterministic, "generic" start vector: a fixed low-discrepancy sequence
/// (steps of an irrational `step`), so it is not orthogonal to the leading
/// axes of any realistic input the way an all-ones or unit vector can be.
fn start_vector(d: usize, step: f64) -> Vec<f64> {
    (0..d)
        .map(|i| ((i as f64 + 1.0) * step).fract() - 0.5 + 1e-3)
        .collect()
}

/// `v` minus its components along the (unit or zero) `basis`.
fn orthogonalize(v: &mut [f64], basis: &[&[f64]]) {
    for u in basis {
        let c = dot(u, v);
        v.iter_mut().zip(u.iter()).for_each(|(a, b)| *a -= c * b);
    }
}

/// `v` scaled to unit length; zero when it is negligible next to `scale`.
fn unit(mut v: Vec<f64>, scale: f64) -> Vec<f64> {
    let n = norm(&v);
    if n <= 1e-12 * scale.max(1e-300) {
        v.iter_mut().for_each(|a| *a = 0.0);
    } else {
        v.iter_mut().for_each(|a| *a /= n);
    }
    v
}

/// The two leading eigenvectors (unit, sign-fixed; zero where the input
/// has no spread left) and eigenvalues of the symmetric `m`.
///
/// Subspace iteration on two vectors, then Rayleigh–Ritz: the pair of
/// axes is found as a plane first, and the 2 × 2 problem inside the plane
/// is solved exactly. Plain power iteration with deflation stalls when
/// the first two eigenvalues are close — common here: three voices
/// roughly equidistant from each other give two near-equal axes — while
/// the plane converges at the rate of λ3/λ2 (voice spread vs. noise).
fn top_two_axes(m: &[Vec<f64>]) -> ([Vec<f64>; 2], [f64; 2]) {
    const PHI: f64 = 0.618_033_988_749_894_9;
    const SQRT2_FRACT: f64 = 0.414_213_562_373_095_1;
    let d = m.len();
    let apply = |v: &[f64]| -> Vec<f64> { m.iter().map(|row| dot(row, v)).collect() };
    let orthonormal = |a: Vec<f64>, mut b: Vec<f64>| -> [Vec<f64>; 2] {
        let scale = norm(&a).max(norm(&b));
        let a = unit(a, scale);
        orthogonalize(&mut b, &[&a]);
        let b = unit(b, scale);
        [a, b]
    };
    let mut q = orthonormal(start_vector(d, PHI), start_vector(d, SQRT2_FRACT));
    for _ in 0..MAX_ITERATIONS {
        let next = orthonormal(apply(&q[0]), apply(&q[1]));
        // How far the plane moved: what of the new basis lies outside the old.
        let mut moved = 0.0f64;
        for v in &next {
            let mut r = v.clone();
            orthogonalize(&mut r, &[&q[0], &q[1]]);
            moved = moved.max(norm(&r));
        }
        q = next;
        if moved < TOLERANCE {
            break;
        }
    }
    // Rayleigh–Ritz: diagonalise T = Qᵀ M Q (2 × 2, symmetric).
    let (mq0, mq1) = (apply(&q[0]), apply(&q[1]));
    let (a, b, c) = (dot(&q[0], &mq0), dot(&q[0], &mq1), dot(&q[1], &mq1));
    let mid = (a + c) / 2.0;
    let half = (((a - c) / 2.0).powi(2) + b * b).sqrt();
    let theta = 0.5 * (2.0 * b).atan2(a - c);
    let (cos, sin) = (theta.cos(), theta.sin());
    let mut v1: Vec<f64> = q[0]
        .iter()
        .zip(&q[1])
        .map(|(x, y)| cos * x + sin * y)
        .collect();
    let mut v2: Vec<f64> = q[0]
        .iter()
        .zip(&q[1])
        .map(|(x, y)| -sin * x + cos * y)
        .collect();
    let (mut l1, mut l2) = ((mid + half).max(0.0), (mid - half).max(0.0));
    // An axis with no spread along it is no axis (rank-1 input).
    let top = l1.max(1e-300);
    if l2 <= 1e-12 * top {
        v2.iter_mut().for_each(|x| *x = 0.0);
        l2 = 0.0;
    }
    if l1 <= 1e-300 {
        v1.iter_mut().for_each(|x| *x = 0.0);
        l1 = 0.0;
    }
    fix_sign(&mut v1);
    fix_sign(&mut v2);
    ([v1, v2], [l1, l2])
}

/// Point the axis so its largest-magnitude loading is positive (the first
/// such loading on a tie): the same data always gives the same picture,
/// not its mirror image.
fn fix_sign(v: &mut [f64]) {
    let mut best = 0usize;
    for (i, a) in v.iter().enumerate() {
        if a.abs() > v[best].abs() + 1e-12 {
            best = i;
        }
    }
    if v.get(best).is_some_and(|a| *a < 0.0) {
        v.iter_mut().for_each(|a| *a = -*a);
    }
}

/// The Voice map of a document: every line with a usable embedding (the
/// length most lines have, finite values) projected with [`project`],
/// then at most `max_points` kept for drawing ([`downsample`]).
pub fn voice_map(file: &SegmentsFile, max_points: usize) -> VoiceMap {
    let usable = |e: &[f32]| !e.is_empty() && e.iter().all(|a| a.is_finite());
    // The embedding size most lines have (a stray odd one is skipped).
    let mut sizes: Vec<(usize, usize)> = Vec::new();
    for e in file.segments.iter().filter_map(|s| s.embedding.as_deref()) {
        if !usable(e) {
            continue;
        }
        match sizes.iter_mut().find(|(len, _)| *len == e.len()) {
            Some((_, c)) => *c += 1,
            None => sizes.push((e.len(), 1)),
        }
    }
    let Some(&(dim, _)) = sizes.iter().max_by_key(|(len, c)| (*c, *len)) else {
        return VoiceMap::default();
    };
    let lines: Vec<_> = file
        .segments
        .iter()
        .filter_map(|s| {
            let e = s.embedding.as_deref()?;
            (e.len() == dim && usable(e)).then_some((s, e))
        })
        .collect();
    let rows: Vec<&[f32]> = lines.iter().map(|(_, e)| *e).collect();
    let p = project(&rows);
    let points: Vec<VoiceMapPoint> = lines
        .iter()
        .zip(&p.coords)
        .map(|((s, _), c)| VoiceMapPoint {
            segment_id: s.id,
            speaker_id: s.speaker_id.clone(),
            x: c[0] as f32,
            y: c[1] as f32,
        })
        .collect();
    let total = points.len();
    VoiceMap {
        points: downsample(points, max_points),
        total,
        explained: p.explained as f32,
    }
}

/// At most `max` points, deterministic: each speaker keeps a share
/// proportional to its lines (at least one while there is room), picked
/// evenly over its lines in document order, so every voice and the whole
/// length of the recording stay on the map. The result keeps the input
/// order.
pub fn downsample(points: Vec<VoiceMapPoint>, max: usize) -> Vec<VoiceMapPoint> {
    if points.len() <= max {
        return points;
    }
    if max == 0 {
        return Vec::new();
    }
    // Groups by speaker, in order of first appearance.
    let mut groups: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let key = p.speaker_id.as_deref();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, idx)) => idx.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    let total = points.len();
    let mut quota: Vec<usize> = groups
        .iter()
        .map(|(_, idx)| {
            let share = ((idx.len() * max) as f64 / total as f64).round() as usize;
            share.max(1).min(idx.len())
        })
        .collect();
    // Rounding (and the minimum of one) can overshoot: trim the largest.
    while quota.iter().sum::<usize>() > max {
        let (k, _) = quota
            .iter()
            .enumerate()
            .max_by_key(|(k, q)| (**q, std::cmp::Reverse(*k)))
            .expect("groups are not empty");
        quota[k] -= 1;
    }
    // …or undershoot: give the room back to the groups with lines left,
    // largest first.
    let mut room = max - quota.iter().sum::<usize>();
    while room > 0 {
        let Some((k, _)) = groups
            .iter()
            .enumerate()
            .filter(|(k, (_, idx))| quota[*k] < idx.len())
            .max_by_key(|(k, (_, idx))| (idx.len() - quota[*k], std::cmp::Reverse(*k)))
        else {
            break;
        };
        quota[k] += 1;
        room -= 1;
    }
    let mut keep = vec![false; total];
    for ((_, idx), q) in groups.iter().zip(&quota) {
        for k in 0..*q {
            // Evenly spaced over the group, centred in each stride.
            let at = (2 * k + 1) * idx.len() / (2 * q);
            keep[idx[at]] = true;
        }
    }
    points
        .into_iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::Segment;
    use crate::speakers::cluster::tests::{sample, voices};

    /// `per` noisy samples of each of `k` voices, interleaved like a
    /// conversation; returns the rows and the voice of each.
    fn clusters(k: usize, per: usize, noise: f32, seed: u64) -> (Vec<Vec<f32>>, Vec<usize>) {
        let (mut rng, v) = voices(k, seed);
        let mut rows = Vec::new();
        let mut truth = Vec::new();
        for i in 0..k * per {
            let s = i % k;
            rows.push(sample(&mut rng, &v[s], noise));
            truth.push(s);
        }
        (rows, truth)
    }

    fn refs(rows: &[Vec<f32>]) -> Vec<&[f32]> {
        rows.iter().map(Vec::as_slice).collect()
    }

    /// Centroid of each voice on the map.
    fn centroids(coords: &[[f64; 2]], truth: &[usize], k: usize) -> Vec<[f64; 2]> {
        let mut c = vec![[0.0; 2]; k];
        let mut n = vec![0usize; k];
        for (p, &s) in coords.iter().zip(truth) {
            c[s][0] += p[0];
            c[s][1] += p[1];
            n[s] += 1;
        }
        c.iter()
            .zip(&n)
            .map(|(c, &n)| [c[0] / n as f64, c[1] / n as f64])
            .collect()
    }

    fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }

    #[test]
    fn separable_voices_stay_separated() {
        for (k, seed) in [(2usize, 11u64), (3, 7), (3, 42)] {
            let (rows, truth) = clusters(k, 60, 1.0, seed);
            let p = project(&refs(&rows));
            assert_eq!(p.coords.len(), rows.len());
            let c = centroids(&p.coords, &truth, k);
            // Every point is nearer its own voice's centroid than any other.
            for (pt, &s) in p.coords.iter().zip(&truth) {
                let own = dist(*pt, c[s]);
                for (o, co) in c.iter().enumerate() {
                    if o != s {
                        assert!(
                            own < dist(*pt, *co),
                            "k={k} seed={seed}: a point sits with another voice"
                        );
                    }
                }
            }
            // And the voices are far apart compared to their spread.
            let spread = p
                .coords
                .iter()
                .zip(&truth)
                .map(|(pt, &s)| dist(*pt, c[s]))
                .fold(0.0, f64::max);
            for a in 0..k {
                for b in a + 1..k {
                    assert!(
                        dist(c[a], c[b]) > spread,
                        "k={k} seed={seed}: voices overlap"
                    );
                }
            }
            assert!(p.explained > 0.0 && p.explained <= 1.0);
        }
    }

    #[test]
    fn projection_is_deterministic() {
        let (rows, _) = clusters(3, 40, 1.0, 5);
        let a = project(&refs(&rows));
        let b = project(&refs(&rows));
        assert_eq!(a, b);
    }

    #[test]
    fn signs_do_not_depend_on_line_order_or_scale() {
        let (rows, _) = clusters(3, 40, 1.0, 9);
        let a = project(&refs(&rows));
        // Reversed order, and every row scaled (normalisation undoes it):
        // each line lands on the same spot, not a mirror image.
        let rev: Vec<Vec<f32>> = rows
            .iter()
            .rev()
            .map(|r| r.iter().map(|x| x * 3.5).collect())
            .collect();
        let b = project(&refs(&rev));
        for (i, pa) in a.coords.iter().enumerate() {
            let pb = b.coords[rows.len() - 1 - i];
            assert!((pa[0] - pb[0]).abs() < 1e-6, "x flipped or moved");
            assert!((pa[1] - pb[1]).abs() < 1e-6, "y flipped or moved");
        }
        // The convention itself: each axis's largest loading is positive,
        // so negating every row gives the mirror image of the points
        // (same axes), not the same picture.
        let neg: Vec<Vec<f32>> = rows
            .iter()
            .map(|r| r.iter().map(|x| -x).collect())
            .collect();
        let c = project(&refs(&neg));
        for (pa, pc) in a.coords.iter().zip(&c.coords) {
            assert!((pa[0] + pc[0]).abs() < 1e-6 && (pa[1] + pc[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn axes_are_orthogonal_and_ordered() {
        let (rows, _) = clusters(3, 50, 1.0, 3);
        let p = project(&refs(&rows));
        let var =
            |k: usize| p.coords.iter().map(|c| c[k] * c[k]).sum::<f64>() / p.coords.len() as f64;
        let cov = p.coords.iter().map(|c| c[0] * c[1]).sum::<f64>() / p.coords.len() as f64;
        assert!(var(0) >= var(1), "the first axis carries the most spread");
        assert!(
            cov.abs() < 1e-6 * var(0).max(1e-12),
            "axes are uncorrelated"
        );
    }

    #[test]
    fn near_equal_axes_are_found_exactly() {
        // λ1 ≈ λ2 (three equidistant voices look like this) stalls plain
        // power iteration; the plane + Rayleigh–Ritz gets both axes.
        let d = 6;
        let mut m = vec![vec![0.0; d]; d];
        for (i, l) in [0.2, 1.0, 0.05, 0.999, 0.1, 0.01].iter().enumerate() {
            m[i][i] = *l;
        }
        let ([v1, v2], [l1, l2]) = top_two_axes(&m);
        assert!((l1 - 1.0).abs() < 1e-9 && (l2 - 0.999).abs() < 1e-9);
        assert!((v1[1] - 1.0).abs() < 1e-6, "first axis is e1: {v1:?}");
        assert!((v2[3] - 1.0).abs() < 1e-6, "second axis is e3: {v2:?}");
    }

    #[test]
    fn degenerate_input_gives_zeros() {
        assert_eq!(project(&[]).coords.len(), 0);
        let one = [1.0f32, 2.0, 3.0];
        let p = project(&[&one]);
        assert_eq!(p.coords, vec![[0.0, 0.0]]);
        assert_eq!(p.explained, 0.0);
        // The same voice print twice (after normalisation): no spread.
        let twice = [2.0f32, 4.0, 6.0];
        let p = project(&[&one, &twice]);
        assert!(p
            .coords
            .iter()
            .all(|c| c[0].abs() < 1e-12 && c[1].abs() < 1e-12));
        // Two distinct points: all the spread is on x.
        let other = [3.0f32, -1.0, 0.5];
        let p = project(&[&one, &other]);
        assert!((p.coords[0][0] + p.coords[1][0]).abs() < 1e-9);
        assert!(p.coords[0][0].abs() > 0.1 && p.coords[0][1].abs() < 1e-9);
        assert!((p.explained - 1.0).abs() < 1e-9);
        // Mismatched sizes are refused as a whole (zeros).
        let short = [1.0f32];
        assert_eq!(project(&[&one, &short]).coords, vec![[0.0; 2]; 2]);
    }

    fn pt(id: u32, sp: &str) -> VoiceMapPoint {
        VoiceMapPoint {
            segment_id: id,
            speaker_id: Some(sp.to_string()),
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn downsample_keeps_every_voice_in_proportion_and_order() {
        // 900 lines of voice 1, 90 of voice 2, 10 of voice 3, interleaved.
        let mut pts = Vec::new();
        for i in 0..1000u32 {
            let sp = match i % 100 {
                0 => "voice:3",
                1..=9 => "voice:2",
                _ => "voice:1",
            };
            pts.push(pt(i, sp));
        }
        let out = downsample(pts.clone(), 100);
        assert_eq!(out.len(), 100);
        let count = |sp: &str| {
            out.iter()
                .filter(|p| p.speaker_id.as_deref() == Some(sp))
                .count()
        };
        assert_eq!(
            (count("voice:1"), count("voice:2"), count("voice:3")),
            (90, 9, 1)
        );
        assert!(
            out.windows(2).all(|w| w[0].segment_id < w[1].segment_id),
            "document order kept"
        );
        // Spread over the whole recording, not just its start.
        assert!(out.first().unwrap().segment_id < 100 && out.last().unwrap().segment_id > 900);
        // Deterministic, and a no-op under the cap.
        assert_eq!(downsample(pts.clone(), 100), out);
        assert_eq!(downsample(pts.clone(), 1000), pts);
        assert!(downsample(pts, 0).is_empty());
    }

    #[test]
    fn downsample_keeps_a_rare_voice_and_never_exceeds_the_cap() {
        let mut pts: Vec<VoiceMapPoint> = (0..50).map(|i| pt(i, "voice:1")).collect();
        pts.push(pt(50, "voice:2"));
        pts.push(pt(51, "voice:3"));
        let out = downsample(pts.clone(), 5);
        assert_eq!(out.len(), 5);
        assert!(out
            .iter()
            .any(|p| p.speaker_id.as_deref() == Some("voice:2")));
        assert!(out
            .iter()
            .any(|p| p.speaker_id.as_deref() == Some("voice:3")));
        // More voices than room: the cap still holds.
        let many: Vec<VoiceMapPoint> = (0..20)
            .map(|i| pt(i, &format!("voice:{}", i + 1)))
            .collect();
        assert_eq!(downsample(many, 7).len(), 7);
        // Lines without a speaker are a group of their own.
        let mut mixed: Vec<VoiceMapPoint> = (0..30).map(|i| pt(i, "voice:1")).collect();
        mixed.push(VoiceMapPoint {
            segment_id: 30,
            speaker_id: None,
            x: 0.0,
            y: 0.0,
        });
        assert!(downsample(mixed, 4).iter().any(|p| p.speaker_id.is_none()));
    }

    fn seg(id: u32, sp: &str, e: Option<Vec<f32>>) -> Segment {
        Segment {
            id,
            start_ms: id as u64 * 1000,
            end_ms: id as u64 * 1000 + 900,
            speaker_id: Some(sp.to_string()),
            text: format!("line {id}"),
            embedding: e,
            ..Default::default()
        }
    }

    #[test]
    fn voice_map_of_a_document() {
        let (rows, truth) = clusters(2, 30, 1.0, 21);
        let mut segments: Vec<Segment> = rows
            .iter()
            .zip(&truth)
            .enumerate()
            .map(|(i, (r, &s))| seg(i as u32, &format!("voice:{}", s + 1), Some(r.clone())))
            .collect();
        // Lines without voice data, a stray size and a broken vector are left out.
        segments.push(seg(100, "you", None));
        segments.push(seg(101, "voice:1", Some(vec![0.5; 8])));
        let mut bad = rows[0].clone();
        bad[3] = f32::NAN;
        segments.push(seg(102, "voice:1", Some(bad)));
        let file = SegmentsFile {
            segments,
            ..Default::default()
        };
        let map = voice_map(&file, MAX_POINTS);
        assert_eq!(map.total, 60);
        assert_eq!(map.points.len(), 60);
        assert!(map.points.iter().all(|p| p.segment_id < 60));
        assert_eq!(map.points[1].speaker_id.as_deref(), Some("voice:2"));
        assert!(map.explained > 0.0);
        // Drawing cap.
        let small = voice_map(&file, 10);
        assert_eq!((small.total, small.points.len()), (60, 10));
        // Nothing to draw.
        assert_eq!(
            voice_map(&SegmentsFile::default(), MAX_POINTS),
            VoiceMap::default()
        );
    }

    #[test]
    fn five_thousand_lines_project() {
        // The size the card is meant for: runs (and stays separated).
        let (rows, truth) = clusters(3, 1700, 1.0, 13);
        let p = project(&refs(&rows));
        assert_eq!(p.coords.len(), 5100);
        let c = centroids(&p.coords, &truth, 3);
        assert!(dist(c[0], c[1]) > 0.1 && dist(c[1], c[2]) > 0.1 && dist(c[0], c[2]) > 0.1);
    }
}

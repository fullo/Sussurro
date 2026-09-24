//! Speaker clustering on L2-normalised embeddings (pure): the online
//! clusterer used while a session runs, offline agglomerative clustering
//! for "Re-detect speakers", and the fold of small clusters into the
//! nearest voice. The algorithms and thresholds come from the Phase 0
//! spike (#107); see [`super`] for the constants.

/// Dot product (= cosine similarity for L2-normalised vectors).
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `v` scaled to unit length (a zero vector stays zero).
pub fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-12 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// Weighted mean of embeddings, L2-normalised. `None` when there is
/// nothing to average (no vectors, zero total weight, mismatched sizes).
pub fn mean_embedding<'a>(items: impl IntoIterator<Item = (&'a [f32], f32)>) -> Option<Vec<f32>> {
    let mut sum: Option<Vec<f32>> = None;
    let mut total = 0f32;
    for (e, w) in items {
        let s = sum.get_or_insert_with(|| vec![0.0; e.len()]);
        if s.len() != e.len() {
            return None;
        }
        s.iter_mut().zip(e).for_each(|(a, b)| *a += b * w);
        total += w;
    }
    let sum = sum?;
    (total > 0.0).then(|| l2_normalize(sum))
}

/// Online (streaming) clustering: each embedding joins the most similar
/// running centroid if the cosine is at least `threshold`, otherwise it
/// opens a new cluster. Cluster indices are assigned in order of first
/// appearance and never change, so "Voice N" labels are stable while a
/// session runs.
#[derive(Debug, Clone)]
pub struct OnlineClusterer {
    threshold: f32,
    /// Running sums of the member embeddings (the centroid's direction).
    sums: Vec<Vec<f32>>,
}

impl OnlineClusterer {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            sums: Vec::new(),
        }
    }

    /// Number of clusters opened so far.
    pub fn len(&self) -> usize {
        self.sums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sums.is_empty()
    }

    /// Assign `e` (L2-normalised) and update its centroid; returns the
    /// cluster index.
    pub fn assign(&mut self, e: &[f32]) -> usize {
        let best = self
            .sums
            .iter()
            .enumerate()
            .filter(|(_, s)| s.len() == e.len())
            .map(|(i, s)| (i, dot(e, &l2_normalize(s.clone()))))
            .fold(None, |best: Option<(usize, f32)>, (i, sim)| match best {
                Some((_, b)) if b >= sim => best,
                _ => Some((i, sim)),
            });
        match best {
            Some((i, sim)) if sim >= self.threshold => {
                self.sums[i].iter_mut().zip(e).for_each(|(a, b)| *a += b);
                i
            }
            _ => {
                self.sums.push(e.to_vec());
                self.sums.len() - 1
            }
        }
    }
}

/// Offline agglomerative clustering, average linkage on cosine similarity:
/// merge the most similar pair until the best similarity drops below
/// `threshold`. Deterministic (ties go to the lowest indices). Returns one
/// label per embedding, numbered by first appearance (`0, 1, …`).
pub fn agglomerative(embs: &[&[f32]], threshold: f32) -> Vec<usize> {
    let n = embs.len();
    let mut sim = vec![vec![0f32; n]; n];
    for i in 0..n {
        for j in i + 1..n {
            let s = dot(embs[i], embs[j]);
            sim[i][j] = s;
            sim[j][i] = s;
        }
    }
    let mut size = vec![1usize; n];
    let mut alive = vec![true; n];
    let mut owner: Vec<usize> = (0..n).collect();
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for i in (0..n).filter(|&i| alive[i]) {
            for j in (i + 1..n).filter(|&j| alive[j]) {
                if best.is_none_or(|(_, _, b)| sim[i][j] > b) {
                    best = Some((i, j, sim[i][j]));
                }
            }
        }
        let Some((a, b, s)) = best else { break };
        if s < threshold {
            break;
        }
        for k in (0..n).filter(|&k| alive[k] && k != a && k != b) {
            let merged = (size[a] as f32 * sim[a][k] + size[b] as f32 * sim[b][k])
                / (size[a] + size[b]) as f32;
            sim[a][k] = merged;
            sim[k][a] = merged;
        }
        size[a] += size[b];
        alive[b] = false;
        owner.iter_mut().filter(|o| **o == b).for_each(|o| *o = a);
    }
    renumber_by_first_appearance(&owner)
}

/// Map arbitrary labels to `0, 1, …` in order of first appearance.
pub fn renumber_by_first_appearance(labels: &[usize]) -> Vec<usize> {
    let mut map: Vec<(usize, usize)> = Vec::new();
    labels
        .iter()
        .map(|l| match map.iter().find(|(k, _)| k == l) {
            Some((_, v)) => *v,
            None => {
                let v = map.len();
                map.push((*l, v));
                v
            }
        })
        .collect()
}

/// Fold small clusters: every cluster holding less than `min_ms` of speech
/// is dissolved and its members join the most similar surviving centroid
/// (keeps "Voice N" from sprouting tiny extra voices, #107). When no
/// cluster reaches `min_ms` (a short document), nothing is folded. Labels
/// keep their numbers (they are not renumbered); `durations_ms` and `embs`
/// run parallel to `labels`.
pub fn fold_small(
    embs: &[&[f32]],
    durations_ms: &[u64],
    labels: &[usize],
    min_ms: u64,
) -> Vec<usize> {
    let nc = labels.iter().max().map_or(0, |m| m + 1);
    let mut dur = vec![0u64; nc];
    for (l, d) in labels.iter().zip(durations_ms) {
        dur[*l] += d;
    }
    let keep: Vec<usize> = (0..nc).filter(|c| dur[*c] >= min_ms).collect();
    if keep.is_empty() || keep.len() == nc {
        return labels.to_vec();
    }
    let centroids: Vec<Option<Vec<f32>>> = keep
        .iter()
        .map(|c| {
            mean_embedding(
                labels
                    .iter()
                    .zip(embs)
                    .filter(|(l, _)| **l == *c)
                    .map(|(_, e)| (*e, 1.0)),
            )
        })
        .collect();
    labels
        .iter()
        .zip(embs)
        .map(|(l, e)| {
            if keep.contains(l) {
                return *l;
            }
            let mut best = (keep[0], f32::MIN);
            for (c, cent) in keep.iter().zip(&centroids) {
                let s = cent.as_deref().map_or(f32::MIN, |v| dot(e, v));
                if s > best.1 {
                    best = (*c, s);
                }
            }
            best.0
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic pseudo-random generator for synthetic embeddings.
    pub(crate) struct Lcg(pub u64);
    impl Lcg {
        pub(crate) fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
        pub(crate) fn vec(&mut self, dim: usize) -> Vec<f32> {
            (0..dim).map(|_| self.next_f32()).collect()
        }
    }

    /// `speakers` random unit "voices" in 256-d and a noisy sample of one:
    /// same-speaker cosine ≈ 0.5–0.6 and cross-speaker ≈ 0, like the
    /// headset clips in #107.
    pub(crate) fn voices(n: usize, seed: u64) -> (Lcg, Vec<Vec<f32>>) {
        let mut rng = Lcg(seed);
        let v = (0..n).map(|_| l2_normalize(rng.vec(256))).collect();
        (rng, v)
    }

    pub(crate) fn sample(rng: &mut Lcg, voice: &[f32], noise: f32) -> Vec<f32> {
        let n = l2_normalize(rng.vec(voice.len()));
        l2_normalize(voice.iter().zip(&n).map(|(v, e)| v + noise * e).collect())
    }

    #[test]
    fn normalize_and_mean() {
        let v = l2_normalize(vec![3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert_eq!(l2_normalize(vec![0.0, 0.0]), vec![0.0, 0.0]);
        let m = mean_embedding([(&[1.0f32, 0.0][..], 1.0), (&[0.0f32, 1.0][..], 1.0)]).unwrap();
        assert!((m[0] - m[1]).abs() < 1e-6 && (dot(&m, &m) - 1.0).abs() < 1e-6);
        assert!(mean_embedding(std::iter::empty()).is_none());
        assert!(mean_embedding([(&[1.0f32][..], 1.0), (&[1.0f32, 2.0][..], 1.0)]).is_none());
    }

    #[test]
    fn online_separates_three_voices_with_stable_indices() {
        let (mut rng, v) = voices(3, 7);
        let mut c = OnlineClusterer::new(crate::speakers::ONLINE_THRESHOLD);
        // Speaker order 0,1,0,2,1,2… — indices follow first appearance.
        let order = [0, 1, 0, 2, 1, 2, 0, 0, 1, 2, 2, 1, 0];
        let got: Vec<usize> = order
            .iter()
            .map(|&s| c.assign(&sample(&mut rng, &v[s], 1.0)))
            .collect();
        assert_eq!(got, order.to_vec());
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn online_threshold_decides_between_join_and_new() {
        let mut c = OnlineClusterer::new(0.5);
        assert_eq!(c.assign(&[1.0, 0.0]), 0);
        // cos 0.6 ≥ 0.5: joins.
        assert_eq!(c.assign(&[0.6, 0.8]), 0);
        // cos to the centroid (≈ 0.89, 0.45) of (-0.6, 0.8) is < 0.5: new.
        assert_eq!(c.assign(&[-0.6, 0.8]), 1);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn agglomerative_finds_the_voices_and_is_deterministic() {
        let (mut rng, v) = voices(4, 11);
        let truth: Vec<usize> = (0..40).map(|i| (i * 7 + i / 3) % 4).collect();
        let embs: Vec<Vec<f32>> = truth
            .iter()
            .map(|&s| sample(&mut rng, &v[s], 0.9))
            .collect();
        let refs: Vec<&[f32]> = embs.iter().map(Vec::as_slice).collect();
        let got = agglomerative(&refs, crate::speakers::REDETECT_THRESHOLD);
        assert_eq!(got, renumber_by_first_appearance(&truth));
        assert_eq!(
            got,
            agglomerative(&refs, crate::speakers::REDETECT_THRESHOLD)
        );
        // A threshold above every similarity keeps everything apart; one
        // below every similarity merges everything.
        assert_eq!(agglomerative(&refs, 1.1), (0..40).collect::<Vec<_>>());
        assert!(agglomerative(&refs, -1.1).iter().all(|&l| l == 0));
        assert!(agglomerative(&[], 0.3).is_empty());
    }

    #[test]
    fn small_clusters_fold_into_the_nearest_voice() {
        let (mut rng, v) = voices(2, 3);
        // Cluster 2 is a 4 s stray of voice 1 (over-split); cluster 3 is
        // a 3 s stray of voice 0.
        let spec = [
            (0, 0, 6000),
            (1, 1, 6000),
            (0, 0, 6000),
            (2, 1, 4000),
            (1, 1, 6000),
            (3, 0, 3000),
        ];
        let embs: Vec<Vec<f32>> = spec
            .iter()
            .map(|&(_, s, _)| sample(&mut rng, &v[s], 0.9))
            .collect();
        let refs: Vec<&[f32]> = embs.iter().map(Vec::as_slice).collect();
        let labels: Vec<usize> = spec.iter().map(|s| s.0).collect();
        let durs: Vec<u64> = spec.iter().map(|s| s.2).collect();
        assert_eq!(
            fold_small(&refs, &durs, &labels, 10_000),
            vec![0, 1, 0, 1, 1, 0]
        );
        // Nothing reaches the minimum: nothing is folded.
        assert_eq!(fold_small(&refs, &durs, &labels, 60_000), labels);
        assert!(fold_small(&[], &[], &[], 10_000).is_empty());
    }

    #[test]
    fn renumbering_follows_first_appearance() {
        assert_eq!(
            renumber_by_first_appearance(&[5, 5, 2, 9, 2, 5]),
            vec![0, 0, 1, 2, 1, 0]
        );
    }
}

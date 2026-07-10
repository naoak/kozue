//! Cross-axis coordinate assignment.
//!
//! Method: mean-neighbor heuristic with exact overlap resolution.
//!
//! Each layer starts packed in order. Then a fixed sequence of sweeps
//! (down, up, down) sets each node's desired position to the mean of its
//! neighbors in the already-swept layer, and resolves overlaps with
//! isotonic regression (pool-adjacent-violators): the layer order is
//! preserved, minimum separations are enforced exactly, and total squared
//! displacement from the desired positions is minimized. The pass is fully
//! deterministic and keeps straight chains perfectly aligned (a single node
//! per layer is always placed exactly at its neighbor's position).

use crate::ordering::Layered;

/// Assign a cross-axis center coordinate to every lnode.
///
/// `gap` is the minimum clearance between adjacent boxes in a layer.
pub(crate) fn assign_cross(lay: &Layered, gap: f64) -> Vec<f64> {
    let n = lay.layer_of.len();
    let mut down_nb: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut up_nb: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in &lay.segs {
        down_nb[v].push(u);
        up_nb[u].push(v);
    }

    // Initial packed positions (centers), in layer order.
    let mut c = vec![0.0f64; n];
    for layer in &lay.order {
        let mut cursor = 0.0;
        for (i, &v) in layer.iter().enumerate() {
            if i > 0 {
                cursor += gap + (lay.cross_size[layer[i - 1]] + lay.cross_size[v]) / 2.0;
            } else {
                cursor = lay.cross_size[v] / 2.0;
            }
            c[v] = cursor;
        }
    }

    let nl = lay.order.len();
    // Fixed sweep sequence: down, up, down (deterministic).
    for pass in 0..3 {
        let down = pass % 2 == 0;
        let layer_ids: Vec<usize> = if down {
            (1..nl).collect()
        } else {
            (0..nl.saturating_sub(1)).rev().collect()
        };
        let nb = if down { &down_nb } else { &up_nb };
        for l in layer_ids {
            relax_layer(&lay.order[l], &lay.cross_size, nb, gap, &mut c);
        }
    }
    c
}

/// Move one layer's nodes toward the mean of their neighbor positions while
/// enforcing minimum separations, preserving order, and minimizing squared
/// displacement (pool-adjacent-violators).
fn relax_layer(layer: &[usize], cross_size: &[f64], nb: &[Vec<usize>], gap: f64, c: &mut [f64]) {
    if layer.is_empty() {
        return;
    }
    let desired: Vec<f64> = layer
        .iter()
        .map(|&v| {
            let ns = &nb[v];
            if ns.is_empty() {
                c[v]
            } else {
                ns.iter().map(|&u| c[u]).sum::<f64>() / ns.len() as f64
            }
        })
        .collect();
    let sep: Vec<f64> = (1..layer.len())
        .map(|i| (cross_size[layer[i - 1]] + cross_size[layer[i]]) / 2.0 + gap)
        .collect();
    let placed = pav(&desired, &sep);
    for (i, &v) in layer.iter().enumerate() {
        c[v] = placed[i];
    }
}

/// Pool-adjacent-violators: given desired centers and minimum separations
/// between consecutive nodes, return centers `out` with
/// `out[i+1] - out[i] >= sep[i]`, preserving order and minimizing
/// `sum((out[i] - desired[i])^2)`.
fn pav(desired: &[f64], sep: &[f64]) -> Vec<f64> {
    let k = desired.len();
    // Offsets so the constraint becomes monotonicity of z_i = c_i - off_i.
    let mut off = vec![0.0f64; k];
    for i in 1..k {
        off[i] = off[i - 1] + sep[i - 1];
    }
    // Blocks of pooled values: (sum, count).
    let mut blocks: Vec<(f64, usize)> = Vec::with_capacity(k);
    for i in 0..k {
        let mut cur = (desired[i] - off[i], 1usize);
        while let Some(&(s, cnt)) = blocks.last() {
            if s / cnt as f64 > cur.0 / cur.1 as f64 {
                blocks.pop();
                cur = (s + cur.0, cnt + cur.1);
            } else {
                break;
            }
        }
        blocks.push(cur);
    }
    let mut out = Vec::with_capacity(k);
    for &(s, cnt) in &blocks {
        let avg = s / cnt as f64;
        for _ in 0..cnt {
            out.push(avg);
        }
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o += off[i];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordering::build;

    #[test]
    fn pav_keeps_feasible_input() {
        let out = pav(&[0.0, 100.0, 200.0], &[50.0, 50.0]);
        assert_eq!(out, vec![0.0, 100.0, 200.0]);
    }

    #[test]
    fn pav_separates_equal_desires_symmetrically() {
        let out = pav(&[10.0, 10.0], &[50.0]);
        assert!((out[0] - (-15.0)).abs() < 1e-9);
        assert!((out[1] - 35.0).abs() < 1e-9);
    }

    #[test]
    fn pav_enforces_separation() {
        let out = pav(&[0.0, 1.0, 2.0, 100.0], &[10.0, 10.0, 10.0]);
        for i in 1..out.len() {
            assert!(out[i] - out[i - 1] >= 10.0 - 1e-9);
        }
    }

    #[test]
    fn straight_chain_stays_aligned() {
        // A 3-node chain with differing widths: all centers equal.
        let sizes = vec![(80.0, 20.0), (40.0, 20.0), (60.0, 20.0)];
        let layers = vec![0, 1, 2];
        let edges = vec![(0, 1), (1, 2)];
        let lay = build(3, &sizes, &layers, &edges);
        let c = assign_cross(&lay, 40.0);
        assert!((c[0] - c[1]).abs() < 1e-9);
        assert!((c[1] - c[2]).abs() < 1e-9);
    }

    #[test]
    fn diamond_is_symmetric() {
        // start -> {a, b} -> end: start and end centered between a and b.
        let sizes = vec![(50.0, 20.0); 4];
        let layers = vec![0, 1, 1, 2];
        let edges = vec![(0, 1), (0, 2), (1, 3), (2, 3)];
        let lay = build(4, &sizes, &layers, &edges);
        let c = assign_cross(&lay, 40.0);
        assert!((c[0] - (c[1] + c[2]) / 2.0).abs() < 1e-9);
        assert!((c[3] - c[0]).abs() < 1e-9);
        assert!(c[2] - c[1] >= 50.0 + 40.0 - 1e-9, "no overlap in the layer");
    }
}

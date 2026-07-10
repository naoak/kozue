//! Dummy-node insertion and barycenter crossing reduction.
//!
//! Edges spanning two or more layers are split into unit-length segments via
//! dummy nodes so that every segment connects adjacent layers. The node order
//! within each layer is then improved with alternating up/down barycenter
//! sweeps, keeping the best (fewest-crossings) order seen. All tie-breaks are
//! stable with respect to the current order, whose initial state is
//! declaration order — so the whole pass is deterministic.

/// Maximum number of down+up sweep rounds.
const MAX_ROUNDS: usize = 8;

/// The layered graph: real nodes `0..n_real` plus dummy nodes appended after.
pub(crate) struct Layered {
    /// Node order within each layer (`order[l][i]` = lnode id).
    pub order: Vec<Vec<usize>>,
    /// Layer index per lnode.
    pub layer_of: Vec<usize>,
    /// Whether the lnode is a dummy (edge routing point).
    pub is_dummy: Vec<bool>,
    /// Extent along the cross axis (width for direction=down).
    pub cross_size: Vec<f64>,
    /// Extent along the main axis (height for direction=down).
    pub main_size: Vec<f64>,
    /// Unit-length segments `(u, v)` with `layer_of[v] == layer_of[u] + 1`.
    pub segs: Vec<(usize, usize)>,
    /// Per input edge (acyclic orientation): the lnode path
    /// `[from, dummies.., to]`.
    pub chains: Vec<Vec<usize>>,
}

/// Build the layered graph, inserting dummy nodes for long edges.
///
/// `sizes[v]` is `(cross_size, main_size)` of real node `v`; `edges` must be
/// in acyclic orientation with `layers[to] > layers[from]`.
pub(crate) fn build(
    n_real: usize,
    sizes: &[(f64, f64)],
    layers: &[usize],
    edges: &[(usize, usize)],
) -> Layered {
    let max_layer = layers.iter().copied().max().unwrap_or(0);
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    let mut layer_of = layers.to_vec();
    let mut is_dummy = vec![false; n_real];
    let mut cross_size: Vec<f64> = sizes.iter().map(|s| s.0).collect();
    let mut main_size: Vec<f64> = sizes.iter().map(|s| s.1).collect();

    for (v, &l) in layers.iter().enumerate() {
        order[l].push(v);
    }

    let mut segs = Vec::new();
    let mut chains = Vec::with_capacity(edges.len());
    for &(u, v) in edges {
        debug_assert!(layers[v] > layers[u], "edge must point to a lower layer");
        let mut chain = vec![u];
        let mut prev = u;
        for (l, layer_order) in order
            .iter_mut()
            .enumerate()
            .take(layers[v])
            .skip(layers[u] + 1)
        {
            let d = layer_of.len();
            layer_of.push(l);
            is_dummy.push(true);
            cross_size.push(0.0);
            main_size.push(0.0);
            layer_order.push(d);
            segs.push((prev, d));
            chain.push(d);
            prev = d;
        }
        segs.push((prev, v));
        chain.push(v);
        chains.push(chain);
    }

    Layered {
        order,
        layer_of,
        is_dummy,
        cross_size,
        main_size,
        segs,
        chains,
    }
}

/// Positions (index within own layer) per lnode.
pub(crate) fn positions(order: &[Vec<usize>], n: usize) -> Vec<usize> {
    let mut pos = vec![0usize; n];
    for layer in order {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
    }
    pos
}

/// Count edge crossings between all adjacent layer pairs for the given
/// positions.
pub(crate) fn count_crossings(lay: &Layered, pos: &[usize]) -> usize {
    let nl = lay.order.len();
    let mut by_layer: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nl];
    for &(u, v) in &lay.segs {
        by_layer[lay.layer_of[u]].push((pos[u], pos[v]));
    }
    let mut total = 0;
    for list in &mut by_layer {
        list.sort_unstable();
        for i in 0..list.len() {
            for j in (i + 1)..list.len() {
                // After sorting, list[j].0 >= list[i].0. Segments sharing the
                // upper endpoint never cross.
                if list[j].0 > list[i].0 && list[j].1 < list[i].1 {
                    total += 1;
                }
            }
        }
    }
    total
}

/// Reduce crossings with alternating barycenter sweeps.
///
/// Runs at most [`MAX_ROUNDS`] down+up rounds, stopping early (and
/// deterministically) as soon as a round fails to improve the crossing count.
/// The best order encountered is kept.
pub(crate) fn reduce_crossings(lay: &mut Layered) {
    let n = lay.layer_of.len();
    let mut down_nb: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut up_nb: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in &lay.segs {
        down_nb[v].push(u);
        up_nb[u].push(v);
    }

    let mut pos = positions(&lay.order, n);
    let mut best = count_crossings(lay, &pos);
    let mut best_order = lay.order.clone();

    for _ in 0..MAX_ROUNDS {
        if best == 0 {
            break;
        }
        let nl = lay.order.len();
        // Down sweep: order each layer by barycenter of the layer above.
        for l in 1..nl {
            sort_layer(&mut lay.order[l], &mut pos, &down_nb);
        }
        // Up sweep: order each layer by barycenter of the layer below.
        for l in (0..nl.saturating_sub(1)).rev() {
            sort_layer(&mut lay.order[l], &mut pos, &up_nb);
        }
        let crossings = count_crossings(lay, &pos);
        if crossings < best {
            best = crossings;
            best_order = lay.order.clone();
        } else {
            break;
        }
    }

    lay.order = best_order;
}

/// Stable-sort one layer by the barycenter (mean position) of each node's
/// neighbors in the fixed layer; nodes without neighbors keep their current
/// position. Updates `pos` for the sorted layer.
fn sort_layer(layer: &mut Vec<usize>, pos: &mut [usize], nb: &[Vec<usize>]) {
    let keys: Vec<f64> = layer
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let ns = &nb[v];
            if ns.is_empty() {
                i as f64
            } else {
                ns.iter().map(|&u| pos[u] as f64).sum::<f64>() / ns.len() as f64
            }
        })
        .collect();
    let mut idx: Vec<usize> = (0..layer.len()).collect();
    // Keys are finite; ties keep the current order (then declaration order,
    // since the initial order is declaration order).
    idx.sort_by(|&a, &b| {
        keys[a]
            .partial_cmp(&keys[b])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let sorted: Vec<usize> = idx.iter().map(|&i| layer[i]).collect();
    *layer = sorted;
    for (i, &v) in layer.iter().enumerate() {
        pos[v] = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_edges_get_dummies() {
        // 0 -> 1 -> 2 -> 3 plus a long edge 0 -> 3 (spans 3 layers).
        let sizes = vec![(10.0, 10.0); 4];
        let layers = vec![0, 1, 2, 3];
        let edges = vec![(0, 1), (1, 2), (2, 3), (0, 3)];
        let lay = build(4, &sizes, &layers, &edges);
        assert_eq!(lay.layer_of.len(), 6, "two dummies expected");
        assert_eq!(lay.chains[3].len(), 4, "chain 0 -> d -> d -> 3");
        assert!(lay.is_dummy[lay.chains[3][1]]);
        assert!(lay.is_dummy[lay.chains[3][2]]);
        assert_eq!(lay.layer_of[lay.chains[3][1]], 1);
        assert_eq!(lay.layer_of[lay.chains[3][2]], 2);
        // Every segment is unit length.
        for &(u, v) in &lay.segs {
            assert_eq!(lay.layer_of[v], lay.layer_of[u] + 1);
        }
    }

    #[test]
    fn barycenter_reduces_crossings() {
        // Two layers x three nodes, fully inverted matching: in declaration
        // order this has 3 crossings; the barycenter sweep removes all.
        let sizes = vec![(10.0, 10.0); 6];
        let layers = vec![0, 0, 0, 1, 1, 1];
        let edges = vec![(0, 5), (1, 4), (2, 3)];
        let mut lay = build(6, &sizes, &layers, &edges);
        let pos = positions(&lay.order, 6);
        let before = count_crossings(&lay, &pos);
        assert_eq!(before, 3);

        reduce_crossings(&mut lay);
        let pos = positions(&lay.order, 6);
        let after = count_crossings(&lay, &pos);
        assert!(after < before, "ordering must reduce crossings");
        assert_eq!(after, 0);
    }

    #[test]
    fn crossing_count_ignores_shared_endpoints() {
        // A fan 0 -> {1, 2}: no crossings regardless of order.
        let sizes = vec![(10.0, 10.0); 3];
        let layers = vec![0, 1, 1];
        let edges = vec![(0, 1), (0, 2)];
        let lay = build(3, &sizes, &layers, &edges);
        let pos = positions(&lay.order, 3);
        assert_eq!(count_crossings(&lay, &pos), 0);
    }

    #[test]
    fn reduction_is_deterministic() {
        let sizes = vec![(10.0, 10.0); 8];
        let layers = vec![0, 0, 0, 0, 1, 1, 1, 1];
        let edges = vec![(0, 7), (1, 6), (2, 5), (3, 4), (0, 4), (3, 7)];
        let run = || {
            let mut lay = build(8, &sizes, &layers, &edges);
            reduce_crossings(&mut lay);
            lay.order
        };
        assert_eq!(run(), run());
    }
}

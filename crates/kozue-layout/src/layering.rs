//! Layer assignment via the longest-path method on an acyclic graph.

/// Assign a layer to each node using the longest path from any source.
///
/// `edges` must be acyclic (guaranteed by [`crate::cycle::greedy_reversed`]).
pub(crate) fn longest_path(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for &(u, v) in edges {
        adj[u].push(v);
        indeg[v] += 1;
    }

    // Kahn topological order, picking the lowest-index ready node so the
    // result is deterministic.
    let mut layer = vec![0usize; n];
    let mut remaining = indeg;
    let mut processed = vec![false; n];
    for _ in 0..n {
        let picked = (0..n).find(|&i| !processed[i] && remaining[i] == 0);
        let Some(u) = picked else {
            // Unreachable for acyclic input; bail out defensively.
            debug_assert!(false, "longest_path called with a cyclic graph");
            break;
        };
        processed[u] = true;
        for &v in &adj[u] {
            if layer[u] + 1 > layer[v] {
                layer[v] = layer[u] + 1;
            }
            remaining[v] -= 1;
        }
    }
    layer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_layers_increase() {
        let layers = longest_path(3, &[(0, 1), (1, 2)]);
        assert_eq!(layers, vec![0, 1, 2]);
    }

    #[test]
    fn longest_path_wins() {
        // 0 -> 1 -> 2 and 0 -> 2: node 2 sits on layer 2, not 1.
        let layers = longest_path(3, &[(0, 1), (1, 2), (0, 2)]);
        assert_eq!(layers, vec![0, 1, 2]);
    }

    #[test]
    fn isolated_nodes_are_layer_zero() {
        let layers = longest_path(3, &[]);
        assert_eq!(layers, vec![0, 0, 0]);
    }
}

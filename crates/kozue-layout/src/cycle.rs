//! Cycle removal: greedy DFS-based back-edge detection.
//!
//! Edges flagged as reversed are flipped for the layout phases only; the
//! final drawing keeps the original direction (arrowheads point along the
//! original edge).

/// Return, for each edge, whether it must be reversed to make the graph
/// acyclic.
///
/// Performs a DFS over nodes in declaration order (index order) and marks
/// every back edge (an edge to a node currently on the DFS stack). Reversing
/// exactly the back edges of a DFS forest always yields a DAG: every non-back
/// edge goes from a higher finishing time to a lower one, and a reversed back
/// edge does too.
pub(crate) fn greedy_reversed(n: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    // Adjacency in edge-declaration order, storing edge indices.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (ei, &(u, _)) in edges.iter().enumerate() {
        adj[u].push(ei);
    }

    const UNVISITED: u8 = 0;
    const ON_STACK: u8 = 1;
    const DONE: u8 = 2;

    let mut state = vec![UNVISITED; n];
    let mut reversed = vec![false; edges.len()];

    for start in 0..n {
        if state[start] != UNVISITED {
            continue;
        }
        // Iterative DFS: (node, next child index).
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = ON_STACK;
        while let Some(top) = stack.last_mut() {
            let (u, ci) = (top.0, top.1);
            if ci < adj[u].len() {
                top.1 += 1;
                let ei = adj[u][ci];
                let v = edges[ei].1;
                if state[v] == ON_STACK {
                    reversed[ei] = true;
                } else if state[v] == UNVISITED {
                    state[v] = ON_STACK;
                    stack.push((v, 0));
                }
            } else {
                state[u] = DONE;
                stack.pop();
            }
        }
    }
    reversed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_acyclic(n: usize, edges: &[(usize, usize)]) -> bool {
        let mut indeg = vec![0usize; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(u, v) in edges {
            adj[u].push(v);
            indeg[v] += 1;
        }
        let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        let mut seen = 0;
        while let Some(u) = ready.pop() {
            seen += 1;
            for &v in &adj[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    ready.push(v);
                }
            }
        }
        seen == n
    }

    fn apply(edges: &[(usize, usize)], rev: &[bool]) -> Vec<(usize, usize)> {
        edges
            .iter()
            .zip(rev)
            .map(|(&(u, v), &r)| if r { (v, u) } else { (u, v) })
            .collect()
    }

    #[test]
    fn dag_has_no_reversals() {
        let edges = vec![(0, 1), (1, 2), (0, 2)];
        let rev = greedy_reversed(3, &edges);
        assert_eq!(rev, vec![false, false, false]);
    }

    #[test]
    fn triangle_cycle_reverses_one_edge() {
        let edges = vec![(0, 1), (1, 2), (2, 0)];
        let rev = greedy_reversed(3, &edges);
        assert_eq!(rev.iter().filter(|&&r| r).count(), 1);
        assert!(is_acyclic(3, &apply(&edges, &rev)));
    }

    #[test]
    fn two_node_cycle_reverses_one_edge() {
        let edges = vec![(0, 1), (1, 0)];
        let rev = greedy_reversed(2, &edges);
        assert_eq!(rev, vec![false, true]);
        assert!(is_acyclic(2, &apply(&edges, &rev)));
    }

    #[test]
    fn nested_cycles_become_acyclic() {
        // Two overlapping cycles: 0->1->2->0 and 1->3->1.
        let edges = vec![(0, 1), (1, 2), (2, 0), (1, 3), (3, 1)];
        let rev = greedy_reversed(4, &edges);
        assert!(is_acyclic(4, &apply(&edges, &rev)));
    }
}

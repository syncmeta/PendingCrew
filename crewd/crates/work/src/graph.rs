//! 有向图找环。边的方向（裁定 4）：父 → 子（父项等子项）、项 → 它依赖的项、项 → 卡住它的项。
use crate::ids::WorkId;
use crate::model::{Blocker, WorkItem};
use std::collections::BTreeMap;

/// 所有项的出边。指向不存在的项的边丢掉（悬空引用由 `invariants` 单独报）。
pub fn adjacency(items: &BTreeMap<WorkId, WorkItem>) -> BTreeMap<WorkId, Vec<WorkId>> {
    let mut adj: BTreeMap<WorkId, Vec<WorkId>> = items.keys().map(|&k| (k, Vec::new())).collect();
    for item in items.values() {
        let out = adj.get_mut(&item.id).expect("key exists");
        for dep in &item.depends_on {
            if items.contains_key(dep) {
                out.push(*dep);
            }
        }
        for b in &item.blocked_on {
            if let Blocker::Item(w) = b
                && items.contains_key(w)
            {
                out.push(*w);
            }
        }
        if let Some(p) = item.parent
            && let Some(children) = adj.get_mut(&p)
        {
            children.push(item.id);
        }
    }
    adj
}

/// 找一个环：返回环上的节点，按边的方向排列（最后一个指回第一个）；无环返回 `None`。
/// 迭代 DFS（深图不爆栈），按 `BTreeMap` 顺序走，结果确定。
pub fn find_cycle(adj: &BTreeMap<WorkId, Vec<WorkId>>) -> Option<Vec<WorkId>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        OnPath,
        Finished,
    }
    let mut mark: BTreeMap<WorkId, Mark> = adj.keys().map(|&k| (k, Mark::New)).collect();
    for &start in adj.keys() {
        if mark[&start] != Mark::New {
            continue;
        }
        mark.insert(start, Mark::OnPath);
        let mut path: Vec<WorkId> = vec![start];
        let mut next_edge: Vec<usize> = vec![0];
        while let Some(&node) = path.last() {
            let i = *next_edge.last().expect("parallel to path");
            let succ = &adj[&node];
            if i < succ.len() {
                *next_edge.last_mut().expect("parallel to path") += 1;
                let to = succ[i];
                match mark.get(&to) {
                    None | Some(Mark::Finished) => {}
                    Some(Mark::OnPath) => {
                        let at = path.iter().position(|&n| n == to).expect("on path");
                        return Some(path[at..].to_vec());
                    }
                    Some(Mark::New) => {
                        mark.insert(to, Mark::OnPath);
                        path.push(to);
                        next_edge.push(0);
                    }
                }
            } else {
                mark.insert(node, Mark::Finished);
                path.pop();
                next_edge.pop();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 确定性的伪随机数（xorshift64*）。测试专用，不引第三方 crate。
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn graph(edges: &[(u32, u32)], nodes: u32) -> BTreeMap<WorkId, Vec<WorkId>> {
        let mut adj: BTreeMap<WorkId, Vec<WorkId>> =
            (1..=nodes).map(|n| (WorkId(n), Vec::new())).collect();
        for &(a, b) in edges {
            adj.get_mut(&WorkId(a)).unwrap().push(WorkId(b));
        }
        adj
    }

    /// 对照答案：有没有一个节点能沿 ≥1 条边走回自己。
    fn has_cycle_oracle(adj: &BTreeMap<WorkId, Vec<WorkId>>) -> bool {
        adj.keys().any(|&start| {
            let mut seen = BTreeSet::new();
            let mut stack: Vec<WorkId> = adj[&start].clone();
            while let Some(n) = stack.pop() {
                if n == start {
                    return true;
                }
                if seen.insert(n)
                    && let Some(next) = adj.get(&n)
                {
                    stack.extend(next.iter().copied());
                }
            }
            false
        })
    }

    fn assert_is_cycle(adj: &BTreeMap<WorkId, Vec<WorkId>>, cycle: &[WorkId]) {
        assert!(!cycle.is_empty());
        let distinct: BTreeSet<WorkId> = cycle.iter().copied().collect();
        assert_eq!(distinct.len(), cycle.len(), "环上节点不重复: {cycle:?}");
        for (i, a) in cycle.iter().enumerate() {
            let b = cycle[(i + 1) % cycle.len()];
            assert!(adj[a].contains(&b), "{a} → {b} 不是一条边: {cycle:?}");
        }
    }

    #[test]
    fn dag_has_no_cycle() {
        let adj = graph(&[(1, 2), (1, 3), (2, 4), (3, 4)], 4);
        assert_eq!(find_cycle(&adj), None);
    }

    #[test]
    fn self_loop_is_a_cycle() {
        let adj = graph(&[(2, 2)], 3);
        assert_eq!(find_cycle(&adj), Some(vec![WorkId(2)]));
    }

    #[test]
    fn long_cycle_is_reported_in_edge_order() {
        let adj = graph(&[(1, 2), (2, 3), (3, 4), (4, 2)], 4);
        assert_eq!(
            find_cycle(&adj),
            Some(vec![WorkId(2), WorkId(3), WorkId(4)])
        );
    }

    #[test]
    fn dangling_targets_are_ignored() {
        let mut adj = graph(&[(1, 2)], 2);
        adj.get_mut(&WorkId(2)).unwrap().push(WorkId(99));
        assert_eq!(find_cycle(&adj), None);
    }

    #[test]
    fn deep_chain_does_not_overflow_the_stack() {
        let n = 50_000u32;
        let edges: Vec<(u32, u32)> = (1..n).map(|i| (i, i + 1)).collect();
        let mut adj = graph(&edges, n);
        assert_eq!(find_cycle(&adj), None);
        adj.get_mut(&WorkId(n)).unwrap().push(WorkId(1));
        assert_eq!(find_cycle(&adj).map(|c| c.len()), Some(n as usize));
    }

    #[test]
    fn property_random_graphs_agree_with_the_oracle() {
        let mut rng = Rng::new(0xC0FFEE);
        let mut with_cycle = 0;
        for _ in 0..3_000 {
            let nodes = 1 + rng.below(9) as u32;
            let edge_count = rng.below(u64::from(nodes) * 2);
            let edges: Vec<(u32, u32)> = (0..edge_count)
                .map(|_| {
                    (
                        1 + rng.below(u64::from(nodes)) as u32,
                        1 + rng.below(u64::from(nodes)) as u32,
                    )
                })
                .collect();
            let adj = graph(&edges, nodes);
            let found = find_cycle(&adj);
            assert_eq!(found.is_some(), has_cycle_oracle(&adj), "{edges:?}");
            if let Some(c) = found {
                with_cycle += 1;
                assert_is_cycle(&adj, &c);
            }
        }
        assert!(with_cycle > 500, "生成器要真的造出环来（{with_cycle}）");
    }

    #[test]
    fn property_topologically_ordered_graphs_never_cycle() {
        let mut rng = Rng::new(7);
        for _ in 0..2_000 {
            let nodes = 2 + rng.below(10) as u32;
            // 随机排列给出一个拓扑序；只连「序靠前 → 序靠后」的边。
            let mut order: Vec<u32> = (1..=nodes).collect();
            for i in (1..order.len()).rev() {
                order.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let mut edges = Vec::new();
            for i in 0..order.len() {
                for j in i + 1..order.len() {
                    if rng.below(3) == 0 {
                        edges.push((order[i], order[j]));
                    }
                }
            }
            assert_eq!(find_cycle(&graph(&edges, nodes)), None, "{edges:?}");
        }
    }
}

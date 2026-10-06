//! 确定性打乱：jev-1.13 偏向第一个选项（官方 jaggedness #8），每次判断把选项顺序打乱。
//! 种子由输入算出（同一输入同一顺序，诊断日志可复现），不引入 rand。
//! SplitMix64（Steele 等，2014）+ Fisher–Yates。

/// FNV-1a 64 位，段与段之间插一个 0xff（UTF-8 里不会出现的字节），避免 ["ab","c"] 与 ["a","bc"] 撞。
pub fn seed_from(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain(std::iter::once(0xff)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// [0, n)。n ≤ 255（官方选项上限），取模偏差可忽略。
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

pub fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = SplitMix64(seed);
    for i in (1..items.len()).rev() {
        let j = rng.below(i + 1);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_order_and_it_is_a_permutation() {
        let mut a: Vec<u32> = (0..10).collect();
        let mut b = a.clone();
        shuffle(&mut a, 42);
        shuffle(&mut b, 42);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
        assert_ne!(a, (0..10).collect::<Vec<_>>(), "种子 42 不该恰好不动");
    }

    #[test]
    fn every_item_leads_about_equally_often() {
        let n = 6;
        let rounds = 6000;
        let mut first = vec![0usize; n];
        for seed in 0..rounds {
            let mut v: Vec<usize> = (0..n).collect();
            shuffle(&mut v, seed_from(&["route_todo", &seed.to_string()]));
            first[v[0]] += 1;
        }
        for (item, count) in first.iter().enumerate() {
            assert!(
                (800..=1200).contains(count),
                "选项 {item} 排第一 {count} 次（期望约 1000）：{first:?}"
            );
        }
    }

    #[test]
    fn seed_separates_segments() {
        assert_ne!(seed_from(&["ab", "c"]), seed_from(&["a", "bc"]));
        assert_ne!(seed_from(&["x"]), seed_from(&["x", ""]));
        assert_eq!(seed_from(&["修", "滚动"]), seed_from(&["修", "滚动"]));
    }

    #[test]
    fn tiny_inputs_are_fine() {
        let mut empty: Vec<u8> = vec![];
        shuffle(&mut empty, 1);
        let mut one = vec![7];
        shuffle(&mut one, 1);
        assert_eq!(one, [7]);
    }
}

// Brute-force cosine (dot on normalized vectors) top-k scan: N x D f32, 8 threads via rayon.
use rayon::prelude::*;
use std::time::Instant;

fn rng(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17;
    (*seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    // 8 accumulators -> LLVM autovectorizes (AVX2/AVX-512 with target-cpu=native)
    let mut acc = [0f32; 16];
    for (ca, cb) in a.chunks_exact(16).zip(b.chunks_exact(16)) {
        for i in 0..16 { acc[i] += ca[i] * cb[i]; }
    }
    acc.iter().sum()
}

fn dot_i8(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(x, y)| *x as i32 * *y as i32).sum()
}

fn main() {
    for &(n, d) in &[(100_000usize, 256usize), (100_000, 768), (1_000_000, 256), (1_000_000, 768)] {
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut m: Vec<f32> = (0..n * d).map(|_| rng(&mut seed)).collect();
        m.par_chunks_mut(d).for_each(|r| { let s = r.iter().map(|x| x*x).sum::<f32>().sqrt(); r.iter_mut().for_each(|x| *x /= s); });
        let q: Vec<f32> = m[..d].to_vec();
        let qi: Vec<i8> = q.iter().map(|x| (x * 127.0 * 8.0).clamp(-127.0, 127.0) as i8).collect();
        let mi: Vec<i8> = m.iter().map(|x| (x * 127.0 * 8.0).clamp(-127.0, 127.0) as i8).collect();
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let mut top: Vec<(usize, f32)> = m.par_chunks(d).enumerate().map(|(i, r)| (i, dot(r, &q))).collect();
            top.select_nth_unstable_by(10, |a, b| b.1.total_cmp(&a.1));
            top.truncate(10);
            best = best.min(t.elapsed().as_secs_f64());
            assert_eq!(top.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0, 0);
        }
        let mut best_i8 = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let mut top: Vec<(usize, i32)> = mi.par_chunks(d).enumerate().map(|(i, r)| (i, dot_i8(r, &qi))).collect();
            top.select_nth_unstable_by(10, |a, b| b.1.cmp(&a.1));
            best_i8 = best_i8.min(t.elapsed().as_secs_f64());
        }
        println!("N={n:>8} D={d:>3}: f32 {:.1} ms/query ({:.0} MB), i8 {:.1} ms/query ({:.0} MB)",
            best * 1e3, (n * d * 4) as f64 / 1e6, best_i8 * 1e3, (n * d) as f64 / 1e6);
    }
}

struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

pub fn layout(n: usize, edges: &[(usize, usize)], iterations: usize, seed: u64) -> Vec<(f32, f32)> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(0.0, 0.0)];
    }
    let mut rng = Rng(seed);
    let mut pos: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let r = 0.5 + 0.5 * rng.next_f32();
            (a.cos() * r, a.sin() * r)
        })
        .collect();
    let edges: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| a != b && a < n && b < n)
        .collect();
    let k = (4.0 / n as f32).sqrt();
    let k2 = k * k;
    let mut temp = 0.2f32;
    let cool = temp / (iterations.max(1) as f32 + 1.0);
    let mut disp = vec![(0.0f32, 0.0f32); n];
    for _ in 0..iterations {
        disp.iter_mut().for_each(|d| *d = (0.0, 0.0));
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                let d2 = (dx * dx + dy * dy).max(1e-6);
                let f = k2 / d2;
                disp[i].0 += dx * f;
                disp[i].1 += dy * f;
                disp[j].0 -= dx * f;
                disp[j].1 -= dy * f;
            }
        }
        for &(a, b) in &edges {
            let dx = pos[a].0 - pos[b].0;
            let dy = pos[a].1 - pos[b].1;
            let d = (dx * dx + dy * dy).sqrt().max(1e-3);
            let f = d / k;
            disp[a].0 -= dx * f;
            disp[a].1 -= dy * f;
            disp[b].0 += dx * f;
            disp[b].1 += dy * f;
        }
        for (p, d) in pos.iter().zip(disp.iter_mut()) {
            d.0 -= p.0 * 0.5;
            d.1 -= p.1 * 0.5;
        }
        for (p, d) in pos.iter_mut().zip(&disp) {
            let len = (d.0 * d.0 + d.1 * d.1).sqrt().max(1e-9);
            let step = len.min(temp);
            p.0 += d.0 / len * step;
            p.1 += d.1 / len * step;
        }
        temp = (temp - cool).max(0.002);
    }
    normalize(&mut pos);
    pos
}

fn normalize(pos: &mut [(f32, f32)]) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pos.iter() {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let half = ((x1 - x0).max(y1 - y0) / 2.0).max(1e-6);
    for p in pos.iter_mut() {
        p.0 = (p.0 - cx) / half;
        p.1 = (p.1 - cy) / half;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn linked_nodes_cluster() {
        let edges = [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)];
        let p = layout(6, &edges, 300, 7);
        let within = dist(p[0], p[1]).max(dist(p[1], p[2])).max(dist(p[3], p[4]));
        let across = dist(p[0], p[3]).min(dist(p[1], p[4])).min(dist(p[2], p[5]));
        assert!(within < across, "within {within} across {across}");
    }

    #[test]
    fn deterministic_bounded_and_finite() {
        let edges: Vec<(usize, usize)> = (1..40).map(|i| (0, i)).chain([(5, 5), (2, 99)]).collect();
        let a = layout(40, &edges, 100, 42);
        assert_eq!(a, layout(40, &edges, 100, 42));
        assert_ne!(a, layout(40, &edges, 100, 43));
        for (x, y) in &a {
            assert!(x.is_finite() && y.is_finite());
            assert!(x.abs() <= 1.0001 && y.abs() <= 1.0001);
        }
        let (hx, hy) = a[0];
        assert!(hx.abs() < 0.35 && hy.abs() < 0.35, "hub at {hx},{hy}");
        assert!(layout(0, &[], 10, 1).is_empty());
        assert_eq!(layout(1, &[], 10, 1), vec![(0.0, 0.0)]);
    }
}

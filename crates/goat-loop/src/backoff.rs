use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DecorrelatedJitter {
    pub floor: Duration,
    pub ceiling: Duration,
    last: Duration,
    state: u64,
}

impl DecorrelatedJitter {
    pub fn new(floor: Duration, ceiling: Duration) -> Self {
        Self {
            floor,
            ceiling,
            last: floor,
            state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn default_self_tick() -> Self {
        Self::new(Duration::from_secs(60), Duration::from_secs(60 * 60))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Duration {
        let max = self.ceiling.min(self.last.saturating_mul(3));
        let max_ms = max.as_millis() as u64;
        let floor_ms = self.floor.as_millis() as u64;
        let span = max_ms.saturating_sub(floor_ms).max(1);
        let r = self.rand() % span;
        self.last = Duration::from_millis(floor_ms + r);
        self.last
    }

    pub fn reset_floor(&mut self) {
        self.last = self.floor;
    }

    fn rand(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_inside_bounds() {
        let mut b = DecorrelatedJitter::default_self_tick();
        for _ in 0..100 {
            let d = b.next();
            assert!(d >= b.floor);
            assert!(d <= b.ceiling);
        }
    }

    #[test]
    fn reset_returns_floor_next() {
        let mut b = DecorrelatedJitter::default_self_tick();
        for _ in 0..5 {
            let _ = b.next();
        }
        b.reset_floor();
        let d = b.next();
        assert!(d <= b.floor.saturating_mul(3));
    }
}

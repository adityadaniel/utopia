//! Interpolation for the painted caret only. Text offsets and IME geometry stay in TextField.

use std::time::{Duration, Instant};

use gpui::{point, Pixels, Point};

use crate::motion::ease_out_cubic;

/// Short enough for the caret to land between most keystrokes (DESIGN.md §2.6).
const CARET_GLIDE: Duration = Duration::from_millis(80);

#[derive(Clone, Copy)]
pub(super) struct Target {
    pub position: Point<Pixels>,
    pub cursor: usize,
    pub line_height: Pixels,
    pub width: Pixels,
}

#[derive(Clone, Copy)]
struct Glide {
    from: Point<Pixels>,
    start: Instant,
}

pub(super) struct CaretMotion {
    target: Target,
    glide: Option<Glide>,
}

impl CaretMotion {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            glide: None,
        }
    }

    /// Retarget from the current animation sample, and stop asking for frames at the destination.
    pub fn update(
        &mut self,
        target: Target,
        now: Instant,
        reduce_motion: impl FnOnce() -> bool,
    ) -> (Point<Pixels>, bool) {
        let old = self.target;
        self.target = target;
        if old.width != target.width || old.line_height != target.line_height {
            self.glide = None;
        } else if old.position != target.position {
            let from = Self::sample(self.glide, old.position, now).0;
            let delta = target.position - from;
            // A long jump (including a wrap across most of the column) should not send
            // the caret streaking over unrelated text.
            let nearby =
                delta.y.abs() <= target.line_height && delta.x.abs() <= target.line_height * 3.;
            self.glide = (old.cursor != target.cursor
                && from != target.position
                && nearby
                && !reduce_motion())
            .then_some(Glide { from, start: now });
        }

        let (position, moving) = Self::sample(self.glide, target.position, now);
        if !moving {
            self.glide = None;
        }
        (position, moving)
    }

    fn sample(glide: Option<Glide>, to: Point<Pixels>, now: Instant) -> (Point<Pixels>, bool) {
        let Some(glide) = glide else {
            return (to, false);
        };
        let elapsed = now - glide.start;
        if elapsed >= CARET_GLIDE {
            return (to, false);
        }

        let t = ease_out_cubic(elapsed.as_secs_f32() / CARET_GLIDE.as_secs_f32());
        let delta = to - glide.from;
        (glide.from + point(delta.x * t, delta.y * t), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn target(x: f32, cursor: usize) -> Target {
        Target {
            position: point(px(x), px(0.)),
            cursor,
            line_height: px(24.),
            width: px(400.),
        }
    }

    /// §13.5 / DESIGN.md §2.6: rapid reversals start from the current drawn position.
    #[test]
    fn retargets_from_the_current_position_and_stops_at_the_destination() {
        let start = Instant::now();
        let left = target(0., 0);
        let right = target(10., 1);
        let mut motion = CaretMotion::new(left);
        assert_eq!(motion.update(right, start, || false), (left.position, true));
        let midway = start + CARET_GLIDE / 2;
        let (in_flight, true) = motion.update(right, midway, || unreachable!()) else {
            panic!("a nearby move should still be running");
        };
        assert!(left.position.x < in_flight.x && in_flight.x < right.position.x);
        assert_eq!(motion.update(left, midway, || false), (in_flight, true));
        assert_eq!(
            motion.update(left, midway + CARET_GLIDE, || unreachable!()),
            (left.position, false),
        );
    }

    /// DESIGN.md §2.6: Reduce Motion, long jumps, and reflow land immediately.
    #[test]
    fn snaps_for_reduce_motion_jumps_and_reflow() {
        let now = Instant::now();
        let mut motion = CaretMotion::new(target(0., 0));
        let nearby = target(10., 1);
        assert_eq!(
            motion.update(nearby, now, || true),
            (nearby.position, false)
        );
        let far = target(200., 20);
        assert_eq!(
            motion.update(far, now, || unreachable!()),
            (far.position, false)
        );
        let reflow = Target {
            width: px(390.),
            ..target(180., 19)
        };
        assert_eq!(
            motion.update(reflow, now, || unreachable!()),
            (reflow.position, false),
        );
    }
}

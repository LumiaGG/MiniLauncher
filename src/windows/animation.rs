use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Threading::*,
};

/// Armed only during transitions. UI/provider messages do not reset its cadence.
pub(super) struct FrameClock {
    pub handle: HANDLE,
    running: bool,
}

impl FrameClock {
    pub fn running(&self) -> bool {
        self.running
    }

    pub unsafe fn new() -> Self {
        let mut handle = CreateWaitableTimerExW(
            std::ptr::null(),
            std::ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        );
        if handle.is_null() {
            handle =
                CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), 0, TIMER_ALL_ACCESS);
        }
        Self {
            handle,
            running: false,
        }
    }

    pub unsafe fn start(&mut self) -> bool {
        if !self.running && !self.handle.is_null() {
            // 120 Hz budget, without changing the system-wide timer resolution.
            self.running =
                SetWaitableTimer(self.handle, &-80_000, 8, None, std::ptr::null(), 0) != 0;
        }
        self.running
    }

    pub unsafe fn stop(&mut self) {
        if self.running {
            CancelWaitableTimer(self.handle);
            // Cancellation does not clear a signal already pending.
            WaitForSingleObject(self.handle, 0);
            self.running = false;
        }
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        unsafe {
            self.stop();
            if !self.handle.is_null() {
                CloseHandle(self.handle);
            }
        }
    }
}

#[derive(Clone)]
enum Easing {
    EaseOut,
    Smooth,
    Exit,
}

/// A finite transition; retargeting starts at the current position.
#[derive(Clone)]
pub(super) struct Tween {
    from: f64,
    to: f64,
    started: Instant,
    duration: Duration,
    easing: Easing,
}

impl Tween {
    pub fn fixed(value: i32) -> Self {
        Self::new(value, value, Instant::now(), 0)
    }

    pub fn new(from: i32, to: i32, now: Instant, milliseconds: u64) -> Self {
        Self {
            from: from as f64,
            to: to as f64,
            started: now,
            duration: Duration::from_millis(milliseconds),
            easing: Easing::EaseOut,
        }
    }

    pub fn smooth(from: i32, to: i32, now: Instant, milliseconds: u64) -> Self {
        let mut tween = Self::new(from, to, now, milliseconds);
        tween.easing = Easing::Smooth;
        tween
    }

    pub fn exit(from: i32, to: i32, now: Instant, milliseconds: u64) -> Self {
        let mut tween = Self::new(from, to, now, milliseconds);
        tween.easing = Easing::Exit;
        tween
    }

    pub fn value(&self, now: Instant) -> i32 {
        let progress = if self.duration.is_zero() {
            1.0
        } else {
            (now.saturating_duration_since(self.started).as_secs_f64()
                / self.duration.as_secs_f64())
            .min(1.0)
        };
        let eased = match self.easing {
            Easing::Smooth => {
                // Zero velocity and acceleration at both ends avoids a sudden kick.
                progress.powi(3) * (progress * (progress * 6.0 - 15.0) + 10.0)
            }
            Easing::EaseOut => 1.0 - (1.0 - progress).powi(3),
            // Start moving immediately and accelerate into dismissal, without
            // settling on a stationary last frame before the HWND disappears.
            Easing::Exit => progress * (0.75 + 0.25 * progress),
        };
        (self.from + (self.to - self.from) * eased).round() as i32
    }

    pub fn active(&self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.started) < self.duration
    }

    pub fn target(&self) -> i32 {
        self.to as i32
    }

    /// Preserve the leader's timeline and displacement at another screen origin.
    pub fn following_at(&self, origin: i32) -> Self {
        let offset = origin as f64 - self.from;
        Self {
            from: origin as f64,
            to: self.to + offset,
            ..self.clone()
        }
    }

    pub fn retarget(&mut self, to: i32, now: Instant) {
        if self.to != to as f64 {
            *self = Self::new(self.value(now), to, now, 100);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_exit_preserves_spacing_and_deadline() {
        let start = Instant::now();
        let host = Tween::exit(300, 312, start, 100);
        let menu = host.following_at(365);
        for millis in [0, 8, 48, 90, 99, 100, 120] {
            let now = start + Duration::from_millis(millis);
            assert_eq!(menu.value(now) - host.value(now), 65);
            assert_eq!(menu.active(now), host.active(now));
        }
        assert_eq!(menu.target(), 377);
    }

    #[test]
    fn frame_clock_starts_once_and_stops_without_an_idle_signal() {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        unsafe {
            let mut clock = FrameClock::new();
            assert!(!clock.handle.is_null());
            assert_eq!(WaitForSingleObject(clock.handle, 0), WAIT_TIMEOUT);
            assert!(clock.start());
            assert!(clock.start());
            assert_eq!(WaitForSingleObject(clock.handle, 1000), WAIT_OBJECT_0);
            clock.stop();
            assert_eq!(WaitForSingleObject(clock.handle, 0), WAIT_TIMEOUT);
            assert!(clock.start());
            assert_eq!(WaitForSingleObject(clock.handle, 1000), WAIT_OBJECT_0);
        }
    }

    #[test]
    fn transition_finishes_and_retargeting_preserves_position() {
        let start = Instant::now();
        let mut tween = Tween::new(0, 100, start, 100);
        assert_eq!(tween.value(start), 0);
        let middle = start + Duration::from_millis(50);
        let position = tween.value(middle);
        assert!((50..100).contains(&position));
        tween.retarget(-20, middle);
        assert_eq!(tween.value(middle), position);
        let end = middle + Duration::from_millis(100);
        assert_eq!(tween.value(end), -20);
        assert!(!tween.active(end));
        assert_eq!(Tween::fixed(42).value(start), 42);
    }

    #[test]
    fn smooth_transition_eases_in_and_settles() {
        let start = Instant::now();
        let tween = Tween::smooth(0, 1000, start, 300);
        assert_eq!(tween.value(start), 0);
        assert!(tween.active(start));
        assert!(tween.value(start + Duration::from_millis(30)) < 20);
        assert_eq!(tween.value(start + Duration::from_millis(150)), 500);
        assert!(tween.value(start + Duration::from_millis(270)) > 980);
        assert_eq!(tween.value(start + Duration::from_millis(300)), 1000);
        assert!(!tween.active(start + Duration::from_millis(300)));
    }

    #[test]
    fn exit_moves_on_the_first_frame_without_a_stationary_tail() {
        let start = Instant::now();
        for distance in [12, 18, 24] {
            let tween = Tween::exit(300, 300 + distance, start, 100);
            assert_eq!(tween.value(start), 300);
            assert!(tween.value(start + Duration::from_millis(8)) > 300);
            let frames: Vec<_> = (0..=6)
                .map(|frame| tween.value(start + Duration::from_millis(frame * 16)))
                .collect();
            assert!(frames.windows(2).all(|pair| pair[1] > pair[0]));
            assert!(tween.value(start + Duration::from_millis(96)) < tween.target());
            let end = start + Duration::from_millis(100);
            assert_eq!(tween.value(end), 300 + distance);
            assert!(!tween.active(end));
        }
    }
}

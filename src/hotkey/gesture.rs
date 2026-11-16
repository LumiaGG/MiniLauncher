//! Pure recognition of two short, isolated taps of the same physical key.
#[derive(Default)]
pub(super) struct Detector {
    pub held: Vec<u32>,
    press: Option<(u32, u32, bool)>,
    first: Option<(u32, u32)>,
}

impl Detector {
    pub fn cancel(&mut self) {
        self.press = None;
        self.first = None;
    }

    pub fn key(&mut self, key: u32, up: bool, time: u32) -> bool {
        if up {
            self.held.retain(|held| *held != key);
            if let Some((pressed, start, second)) = self.press.take() {
                if pressed == key && time.wrapping_sub(start) <= 250 {
                    if second {
                        self.first = None;
                        return true;
                    }
                    self.first = Some((key, start));
                } else {
                    self.first = None;
                }
            }
        } else {
            if self.held.contains(&key) {
                return false; // OS auto-repeat is not a new tap.
            }
            let isolated = self.held.is_empty();
            self.held.push(key);
            if !isolated {
                self.cancel();
                return false;
            }
            let second = self
                .first
                .take()
                .is_some_and(|(first, start)| first == key && time.wrapping_sub(start) <= 350);
            self.press = Some((key, time, second));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(d: &mut Detector, key: u32, time: u32) -> bool {
        assert!(!d.key(key, false, time));
        d.key(key, true, time.wrapping_add(30))
    }

    #[test]
    fn double_taps_fire_on_release_and_do_not_overlap() {
        for key in [0xa2, 0xa3, 0xa0, 0xa4, 0x5b, 0x41, 0x20, 0x70] {
            let mut d = Detector::default();
            assert!(!tap(&mut d, key, 0));
            assert!(tap(&mut d, key, 150));
            assert!(!tap(&mut d, key, 250));
        }
    }

    #[test]
    fn interruptions_sides_long_holds_and_timeouts_cancel() {
        let mut d = Detector::default();
        assert!(!tap(&mut d, 0xa2, 0));
        assert!(!tap(&mut d, 0xa3, 100));
        assert!(!tap(&mut d, 0xa3, 500));
        d.cancel(); // Mouse activity or injected input.
        assert!(!tap(&mut d, 0xa3, 600));
        d.key(0x43, false, 650);
        assert!(!tap(&mut d, 0xa3, 700));
        d.key(0x43, true, 750);
        d.key(0xa3, false, 800);
        assert!(!d.key(0xa3, false, 850));
        assert!(!d.key(0xa3, true, 1100));
        assert!(!tap(&mut d, 0xa3, 1150));
    }

    #[test]
    fn timestamp_wrap_is_supported() {
        let mut d = Detector::default();
        assert!(!tap(&mut d, 0x41, u32::MAX - 100));
        assert!(tap(&mut d, 0x41, 20));
    }
}

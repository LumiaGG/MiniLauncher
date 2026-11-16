//! Opt-in timing; records phases/ids, never query text or plugin content.
use std::{sync::OnceLock, time::Instant};

pub struct Span {
    phase: &'static str,
    detail: String,
    start: Instant,
}

pub fn timing(phase: &'static str, detail: &str) -> Option<Span> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED
        .get_or_init(|| std::env::var("MINILAUNCHER_TRACE_PLUGIN_TIMING").as_deref() == Ok("1"))
    {
        return None;
    }
    Some(Span {
        phase,
        detail: detail.into(),
        start: Instant::now(),
    })
}
impl Drop for Span {
    fn drop(&mut self) {
        eprintln!(
            "mini-launcher timing phase={} detail={} elapsed_us={}",
            self.phase,
            self.detail,
            self.start.elapsed().as_micros()
        );
    }
}

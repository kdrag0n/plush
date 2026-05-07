use std::time::Instant;

pub struct StartupProfile {
    enabled: bool,
    start: Instant,
    last: Instant,
}

impl StartupProfile {
    pub fn from_env() -> Self {
        let now = Instant::now();
        Self {
            enabled: std::env::var_os("PLUSH_PROFILE_STARTUP").is_some(),
            start: now,
            last: now,
        }
    }

    pub fn mark(&mut self, label: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let total = now.duration_since(self.start).as_secs_f64() * 1000.0;
        let delta = now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.last = now;
        eprintln!("plush startup: {total:8.2}ms +{delta:7.2}ms {label}");
    }
}

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct RateLimits(Mutex<HashMap<String, (Instant, usize)>>);

impl RateLimits {
    pub(super) fn take(&self, key: String, limit: usize, window: Duration) -> Result<(), String> {
        let now = Instant::now();
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, (expires, _)| *expires > now);
        if entries.len() >= 10_000 && !entries.contains_key(&key) {
            return Err("请求过于频繁，请稍后再试".into());
        }
        let entry = entries.entry(key).or_insert((now + window, 0));
        if entry.1 >= limit {
            return Err("请求过于频繁，请稍后再试".into());
        }
        entry.1 += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_are_independent_and_expired_entries_can_be_reused() {
        let rates = RateLimits::default();
        let window = Duration::from_secs(60);
        assert!(rates.take("login:a".into(), 2, window).is_ok());
        assert!(rates.take("login:a".into(), 2, window).is_ok());
        assert!(rates.take("login:a".into(), 2, window).is_err());
        assert!(rates.take("login:b".into(), 2, window).is_ok());
        assert!(rates.take("expired".into(), 1, Duration::ZERO).is_ok());
        assert!(rates.take("expired".into(), 1, window).is_ok());
        assert!(rates.take("expired".into(), 1, window).is_err());
    }
}

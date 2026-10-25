use crate::{PingCreationError, PingOptions, PingResult, Pinger};
use rand::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;

/// Made-up but plausible pings, for demos and screenshots (`PINGER_FAKE_PING=1`).
///
/// Each target gets its own baseline latency, derived from its name so a given target
/// always looks the same. The latency drifts slowly around that baseline, spikes now and
/// then, and an occasional ping is lost (`PINGER_FAKE_LOSS`, a ratio, defaults to 0.01).
pub struct FakePinger {
    options: PingOptions,
}

impl Pinger for FakePinger {
    fn from_options(options: PingOptions) -> Result<Self, PingCreationError>
    where
        Self: Sized,
    {
        Ok(Self { options })
    }

    fn parse_fn(&self) -> fn(String) -> Option<PingResult> {
        unimplemented!("parse for FakeParser not implemented")
    }

    fn ping_args(&self) -> (&str, Vec<String>) {
        unimplemented!("ping_args not implemented for FakePinger")
    }

    fn start(&self) -> Result<Receiver<PingResult>, PingCreationError> {
        let (tx, rx) = mpsc::channel();
        let sleep_time = self.options.interval;
        let loss: f64 = std::env::var("PINGER_FAKE_LOSS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.01);

        let mut hasher = DefaultHasher::new();
        self.options.target.to_string().hash(&mut hasher);
        let seed = hasher.finish();

        thread::spawn(move || {
            let mut random = StdRng::seed_from_u64(seed);
            let baseline = 6.0 + (seed % 48) as f64;
            let mut level = baseline;
            let mut spike = 0.0f64;
            loop {
                level += (random.random::<f64>() - 0.5) * baseline * 0.06;
                level = level.clamp(baseline * 0.85, baseline * 1.5);
                if spike < 1.0 && random.random::<f64>() < 0.02 {
                    spike = baseline * random.random_range(1.5..5.0);
                }
                spike *= 0.6;
                let ms = level + spike + random.random::<f64>() * baseline * 0.08;

                let result = if random.random::<f64>() < loss {
                    PingResult::Timeout("Fake ping timeout".to_string())
                } else {
                    PingResult::Pong(
                        Duration::from_secs_f64(ms / 1_000.0),
                        format!("Fake ping line: {ms:.3} ms"),
                    )
                };
                if tx.send(result).is_err() {
                    break;
                }

                thread::sleep(sleep_time);
            }
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_pings_are_plausible() {
        let options = PingOptions::new("192.0.2.1", Duration::from_millis(1), None);
        let pinger = FakePinger::from_options(options).unwrap();
        let results: Vec<PingResult> = pinger.start().unwrap().into_iter().take(300).collect();
        let replies: Vec<f64> = results
            .iter()
            .filter_map(|r| match r {
                PingResult::Pong(rtt, _) => Some(rtt.as_secs_f64() * 1_000.0),
                _ => None,
            })
            .collect();
        assert!(replies.len() > 250, "{} replies", replies.len());
        assert!(replies.iter().all(|ms| (1.0..1_000.0).contains(ms)));
        let spread = replies.iter().copied().fold(0.0, f64::max) - replies.iter().copied().fold(f64::MAX, f64::min);
        assert!(spread > 1.0, "the latency should move around");
    }
}

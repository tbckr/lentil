//! I/O boundaries for time and randomness, replaced by fakes in tests.

use jiff::Timestamp;

/// The current time.
pub trait Clock {
    /// Now, in UTC.
    fn now(&self) -> Timestamp;
}

/// A source of random bytes.
pub trait Random {
    /// Fills `buf` with random bytes.
    fn fill(&mut self, buf: &mut [u8]);
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A fixed clock for tests.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// Randomness from the operating system (`/dev/urandom`).
#[derive(Debug, Default)]
pub struct OsRandom;

impl Random for OsRandom {
    fn fill(&mut self, buf: &mut [u8]) {
        use std::io::Read;
        let filled = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf));
        // Without an OS random source no ID can be unique; stop rather than
        // write colliding IDs.
        if let Err(err) = filled {
            panic!("cannot read /dev/urandom: {err}");
        }
    }
}

/// A deterministic byte sequence for tests: 0, 1, 2, … wrapping at 255.
#[derive(Clone, Debug, Default)]
pub struct CountingRandom(pub u8);

impl Random for CountingRandom {
    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf {
            *b = self.0;
            self.0 = self.0.wrapping_add(1);
        }
    }
}

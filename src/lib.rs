//! An implementation of the
//! [Hybrid Logical Clock](http://www.cse.buffalo.edu/tech-reports/2014-04.pdf)
//! for Rust.

extern crate time;

use std::fmt::{Formatter, Display, Error};
use std::sync::Mutex;

/// The `HLTimespec` type stores a hybrid logical timestamp (also called
/// timespec for symmetry with time::Timespec).
///
/// Such a timestamp is comprised of an "ordinary" wall time and
/// a logical component. Timestamps are compared by wall time first,
/// logical second.
///
/// # Examples
///
/// ```
/// use hlc::HLTimespec;
/// let early = HLTimespec::new(1, 0, 0);
/// let middle = HLTimespec::new(1, 1, 0);
/// let late = HLTimespec::new(1, 1, 1);
/// assert!(early < middle && middle < late);
/// ```
#[derive(Debug,Clone,Copy,Eq,PartialEq,PartialOrd,Ord)]
pub struct HLTimespec {
    wall: time::Timespec,
    logical: u16,
}

impl HLTimespec {
    /// Creates a new hybrid logical timestamp with the given seconds,
    /// nanoseconds, and logical ticks.
    ///
    /// # Examples
    ///
    /// ```
    /// use hlc::HLTimespec;
    /// let ts = HLTimespec::new(1, 2, 3);
    /// assert_eq!(format!("{}", ts), "1.2+3");
    /// ```
    pub fn new(s: i64, ns: i32, l: u16) -> HLTimespec {
        HLTimespec { wall: time::Timespec { sec: s, nsec: ns }, logical: l }
    }
}

impl Display for HLTimespec {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        f.write_str(&format!("{}.{}+{}", self.wall.sec, self.wall.nsec, self.logical))
    }
}

/// `State` is a hybrid logical clock.
///
/// # Examples
///
/// ```
/// use hlc::{HLTimespec, State};
/// let mut s = State::new();
/// println!("{}", s.get_time()); // attach to outgoing event
/// let ext_event_ts = HLTimespec::new(12345, 67, 89); // external event's timestamp
/// let ext_event_recv_ts = s.update(ext_event_ts);
/// ```
///
/// If access to the clock isn't serializable, a convenience method returns
/// a `State` wrapped in a `Mutex`:
///
/// ```
/// use hlc::State;
/// let mut mu = State::new_sendable();
/// {
///     let mut s = mu.lock().unwrap();
///     s.get_time();
/// }
/// ```
pub struct State<F> {
    s: HLTimespec,
    now: F,
}

impl State<()> {
    // Creates a standard hybrid logical clock, using `time::get_time` as
    // supplier of the physical clock's wall time.
    pub fn new() -> State<fn() -> time::Timespec> {
        State::new_with(time::get_time)
    }

    // Returns the result of `State::new()`, wrapped in a `Mutex`.
    pub fn new_sendable() -> Mutex<State<fn() -> time::Timespec>> {
        Mutex::new(State::new())
    }
}

impl<F: FnMut() -> time::Timespec> State<F> {
    /// Creates a hybrid logical clock with the supplied wall time. This is
    /// useful for tests or settings in which an alternative clock is used.
    ///
    /// # Examples
    ///
    /// ```
    /// # extern crate hlc;
    /// # extern crate time;
    /// # fn main() {
    /// use hlc::{HLTimespec, State};
    /// let mut times = vec![time::Timespec { sec: 42, nsec: 9919 }];
    /// let mut s = State::new_with(move || times.pop().unwrap());
    /// let mut ts = s.get_time();
    /// assert_eq!(format!("{}", ts), "42.9919+0");
    /// # }
    /// ```
    pub fn new_with(now: F) -> State<F> {
        State {
            s: HLTimespec { wall: time::Timespec { sec: 0, nsec: 0 }, logical: 0 },
            now: now,
        }
    }

    /// Generates a timestamp from the clock.
    pub fn get_time(&mut self) -> HLTimespec {
        let s = &mut self.s;
        let wall = (self.now)();
        if s.wall < wall {
            s.wall = wall;
            s.logical = 0;
        } else {
            s.logical += 1;
        }
        s.clone()
    }

    /// Assigns a timestamp to an event which happened at the given timestamp
    /// on a remote system.
    pub fn update(&mut self, event: HLTimespec) -> HLTimespec {
        let (wall, s) = ((self.now)(), &mut self.s);

        if wall > event.wall && wall > s.wall {
            s.wall = wall;
            s.logical = 0
        } else if event.wall > s.wall {
            s.wall = event.wall;
            s.logical = event.logical+1;
        } else if s.wall > event.wall {
            s.logical += 1;
        } else {
            if event.logical > s.logical {
                s.logical = event.logical;
            }
            s.logical += 1;
        }
        s.clone()
    }
}

#[cfg(test)]
mod tests {
    extern crate time;
    use {HLTimespec, State};

    fn ts(s: i64, ns: i32) -> time::Timespec {
        time::Timespec { sec: s, nsec: ns }
    }

    fn hlts(s: i64, ns: i32, l: u16) -> HLTimespec {
        HLTimespec::new(s, ns, l)
    }

    #[test]
    fn it_works() {
        let zero = hlts(0, 0, 0);
        let ops = vec![
            // Test cases in the form (wall, event_ts, outcome).
            // Specifying event_ts as zero corresponds to calling `get_time`,
            // otherwise `update`.
            (ts(1,0), zero, hlts(1,0,0)),
            (ts(1,0), zero, hlts(1,0,1)), // clock didn't move
            (ts(0,9), zero, hlts(1,0,2)), // clock moved back
            (ts(2,0), zero, hlts(2,0,0)), // finally ahead again
            (ts(3,0), hlts(1,2,3), hlts(3,0,0)), // event happens, but wall ahead
            (ts(3,0), hlts(1,2,3), hlts(3,0,1)), // event happens, wall ahead but unchanged
            (ts(3,0), hlts(3,0,1), hlts(3,0,2)), // event happens at wall, which is still unchanged
            (ts(3,0), hlts(3,0,99), hlts(3,0,100)), // event with larger logical, wall unchanged
            (ts(3,5), hlts(4,4,100), hlts(4,4,101)), // event with larger wall, our wall behind
            (ts(5,0), hlts(4,5,0), hlts(5,0,0)), // event behind wall, but ahead of previous state
            (ts(4,9), hlts(5,0,99), hlts(5,0,100)),
            (ts(0,0), hlts(5,0,50), hlts(5,0,101)), // event at state, lower logical than state
        ];

        // Prepare fake clock and create State.
        let mut times = ops.iter().rev().map(|op| op.0).collect::<Vec<time::Timespec>>();
        let mut s = State::new_with(move || times.pop().unwrap());

        for op in &ops {
            let t = if op.1 == zero {
                s.get_time()
            } else {
                s.update(op.1.clone())
            };
            assert_eq!(t, op.2);
        }
    }
}

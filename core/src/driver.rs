//! The driver: runs the [`Core`] on its own thread so the UI never waits on it.
//!
//! The UI layer sends [`Input`]s through a [`CoreHandle`] and receives an
//! [`Update`] (the new [`View`] and the [`Effect`]s to perform) after the
//! core starts and after every input. The driver also sends
//! [`Event::Tick`] every [`Core::tick_interval`] ([`TICK_INTERVAL`] unless
//! the core is waiting on something). Inputs are handled one at a time, in
//! order, so the core itself stays single-threaded.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::Core;
use crate::interface::{Command, Effect, Event, View};

/// How often the driver sends [`Event::Tick`].
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Something for the core to handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Command(Command),
    Event(Event),
}

/// What the UI should show and do after the core handled an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub view: View,
    pub effects: Vec<Effect>,
}

/// Sends inputs to the core thread. Dropping every handle stops the thread.
#[derive(Debug, Clone)]
pub struct CoreHandle {
    inputs: Sender<Input>,
}

impl CoreHandle {
    pub fn command(&self, command: Command) {
        // A send only fails once the core thread is gone (the app is quitting).
        let _ = self.inputs.send(Input::Command(command));
    }

    pub fn event(&self, event: Event) {
        let _ = self.inputs.send(Input::Event(event));
    }
}

/// Moves `core` to its own thread. `on_update` is called on that thread, first
/// with the initial view, then after every input; the UI layer hops to the
/// main thread inside it.
pub fn start(core: Core, on_update: impl Fn(Update) + Send + 'static) -> CoreHandle {
    let (inputs, received) = mpsc::channel::<Input>();
    thread::Builder::new()
        .name("accroster-core".into())
        .spawn(move || {
            let mut core = core;
            on_update(Update {
                view: core.view(),
                effects: Vec::new(),
            });
            let mut next_tick = Instant::now() + core.tick_interval();
            loop {
                let wait = next_tick.saturating_duration_since(Instant::now());
                let input = match received.recv_timeout(wait) {
                    Ok(input) => input,
                    Err(RecvTimeoutError::Timeout) => {
                        next_tick = Instant::now() + core.tick_interval();
                        Input::Event(Event::Tick)
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                let effects = match input {
                    Input::Command(command) => core.command(command),
                    Input::Event(event) => core.event(event),
                };
                // An input may have started something the core must check on
                // sooner (a sign-in).
                next_tick = next_tick.min(Instant::now() + core.tick_interval());
                on_update(Update {
                    view: core.view(),
                    effects,
                });
            }
        })
        .expect("start the core thread");
    CoreHandle { inputs }
}

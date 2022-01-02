//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use std::ops::Sub;
use std::time::Duration;
use std::time::Instant;

use crate::graph::Build;
use crate::graph::BuildId;
use crate::work::BuildState;

pub trait Progress {
    fn build_state(&mut self, id: BuildId, build: &Build, prev: BuildState, state: BuildState);
    fn tick(&mut self);
}

pub struct RcProgress<P: Progress> {
    inner: std::rc::Rc<std::cell::RefCell<P>>,
}

impl<P: Progress> RcProgress<P> {
    pub fn new(p: P) -> Self {
        RcProgress {
            inner: std::rc::Rc::new(std::cell::RefCell::new(p)),
        }
    }
}

impl<P: Progress> Progress for RcProgress<P> {
    fn build_state(&mut self, id: BuildId, build: &Build, prev: BuildState, state: BuildState) {
        self.inner.borrow_mut().build_state(id, build, prev, state);
    }
    fn tick(&mut self) {
        self.inner.borrow_mut().tick();
    }
}

pub struct ConsoleProgress {
    last_update: Instant,
    want: usize,
    ready: usize,
    running: usize,
    done: usize,
}

impl ConsoleProgress {
    pub fn new() -> Self {
        ConsoleProgress {
            last_update: Instant::now().sub(Duration::from_secs(1)),
            want: 0,
            ready: 0,
            running: 0,
            done: 0,
        }
    }
}

impl Progress for ConsoleProgress {
    fn build_state(&mut self, _id: BuildId, build: &Build, prev: BuildState, state: BuildState) {
        match prev {
            BuildState::Want => self.want -= 1,
            BuildState::Ready => self.ready -= 1,
            BuildState::Running => self.running -= 1,
            _ => {}
        }
        match state {
            BuildState::Want => self.want += 1,
            BuildState::Ready => self.ready += 1,
            BuildState::Running => {
                if let Some(desc) = &build.desc {
                    println!("{}", desc);
                } else if let Some(cmdline) = &build.cmdline {
                    println!("$ {}", cmdline);
                }
                self.running += 1;
            }
            BuildState::Done => self.done += 1,
            _ => {}
        }
        self.maybe_print();
    }

    fn tick(&mut self) {
        self.maybe_print();
    }
}

impl ConsoleProgress {
    fn render(&self) -> String {
        let total = self.done + self.running + self.ready + self.want;

        let mut out = String::new();
        let mut sum: usize = 0;
        for &(count, ch) in &[
            (self.done, '='),
            (self.running, '*'),
            (self.ready, '-'),
            (self.want, ' '),
        ] {
            sum += count;
            while out.len() <= (sum * 40 / total) {
                out.push(ch);
            }
        }
        out.insert(0, '[');
        out.push(']');
        out.push_str(&format!(
            " [{} {} {} {}]",
            self.done, self.ready, self.running, self.want
        ));
        out
    }

    fn maybe_print(&mut self) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_update);
        if delta < Duration::from_millis(200) {
            return;
        }
        println!("{}", self.render());
        self.last_update = now;
    }
}

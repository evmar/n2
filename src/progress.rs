//! Build progress tracking and reporting, for the purpose of display to the
//! user.

use crate::graph::Build;
use crate::graph::BuildId;

pub struct Progress {
    want: usize,
    ready: usize,
    running: usize,
    done: usize,
}

impl Progress {
    pub fn new() -> Self {
        Progress {
            want: 0,
            ready: 0,
            running: 0,
            done: 0,
        }
    }

    pub fn want(&mut self, _id: BuildId, _build: &Build) {
        self.want += 1;
        self.maybe_print();
    }
    pub fn ready(&mut self, _id: BuildId, _build: &Build) {
        self.want -= 1;
        self.ready += 1;
        self.maybe_print();
    }
    pub fn start(&mut self, _id: BuildId, build: &Build) {
        if let Some(cmdline) = &build.cmdline {
            println!("$ {}", cmdline);
        }

        self.ready -= 1;
        self.running += 1;
        self.maybe_print();
    }
    pub fn finish(&mut self, _id: BuildId, _build: &Build) {
        self.running -= 1;
        self.done += 1;
        self.maybe_print();
    }

    pub fn render(&self) -> String {
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

    fn maybe_print(&self) {
        //println!("{}", self.render());
    }
}

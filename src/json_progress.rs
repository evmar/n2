//! Writes progress updates as JSON messages to a stream.

extern crate json;

use std::io::Write;

use crate::densemap::Index;
use crate::graph::{Build, BuildId};
use crate::progress::Progress;
use crate::task::TaskResult;
use crate::work::{BuildState, StateCounts};

/// Implements progress::Progress by forwarding messages as JSON to a stream.
pub struct JSONProgress {
    stream: Option<Box<dyn Write>>,
}

impl JSONProgress {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let stream = Box::new(std::fs::OpenOptions::new().append(true).open(path)?);
        Ok(JSONProgress {
            stream: Some(stream),
        })
    }

    fn write(&mut self, val: json::JsonValue) {
        if let Some(stream) = &mut self.stream {
            let mut buf = json::stringify(val);
            buf.push('\n');
            if stream.write_all(buf.as_bytes()).is_err() {
                self.stream = None;
            }
        }
    }
}

impl Progress for JSONProgress {
    fn update(&mut self, counts: &StateCounts) {
        self.write(json::object! {
            counts: {
                want: counts.get(BuildState::Want),
                ready: counts.get(BuildState::Ready),
                queued: counts.get(BuildState::Queued),
                running: counts.get(BuildState::Running),
                done: counts.get(BuildState::Done),
                failed: counts.get(BuildState::Failed),
            }
        });
    }

    fn flush(&mut self) {}

    fn task_state(&mut self, id: BuildId, _build: &Build, _result: Option<&TaskResult>) {
        self.write(json::object! {
            task: {
                id: id.index(),
            }
        });
    }

    fn finish(&mut self) {
        // TODO a build finish()es multiple times due to updating build.ninja,
        // so this isn't so useful for giving a status message.
    }
}

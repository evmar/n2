//! Chrome trace output.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

static mut TRACE: Option<Trace> = None;

enum EventType {
    Complete(Instant),
}

struct Event {
    name: &'static str,
    timestamp: Instant,
    event_type: EventType,
}

struct Trace {
    start: Instant,
    w: BufWriter<File>,
}

impl Trace {
    fn new(path: &str) -> std::io::Result<Self> {
        let mut w = BufWriter::new(File::create(path)?);
        write!(w, "[\n")?;
        Ok(Trace {
            start: Instant::now(),
            w,
        })
    }

    fn write_event(&mut self, event: Event) -> std::io::Result<()> {
        write!(
            self.w,
            "{{ \"pid\": 0, \"name\": {:?}, \"ts\": {},",
            event.name,
            event.timestamp.duration_since(self.start).as_micros(),
        )?;
        match event.event_type {
            EventType::Complete(end) => {
                write!(
                    self.w,
                    "\"ph\": \"X\", \"dur\": {} }}",
                    end.duration_since(event.timestamp).as_micros()
                )
            }
        }
    }

    fn write(&mut self, event: Event) -> std::io::Result<()> {
        self.write_event(event)?;
        write!(self.w, ",\n")
    }

    fn scope<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let result = f();
        self.write(Event {
            name,
            timestamp: start,
            event_type: EventType::Complete(Instant::now()),
        })
        .unwrap();
        result
    }

    fn close(&mut self) -> std::io::Result<()> {
        self.write_event(Event {
            name: "main",
            timestamp: self.start,
            event_type: EventType::Complete(Instant::now()),
        })?;
        write!(self.w, "]\n")?;
        self.w.flush()
    }
}

pub fn open(path: &str) -> std::io::Result<()> {
    let trace = Trace::new(path)?;
    // Safety: accessing global mut, not threadsafe.
    unsafe {
        TRACE = Some(trace);
    }
    Ok(())
}

#[inline]
pub fn scope<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    // Safety: accessing global mut, not threadsafe.
    unsafe {
        match &mut TRACE {
            None => f(),
            Some(t) => t.scope(name, f),
        }
    }
}

pub fn close() -> std::io::Result<()> {
    // Safety: accessing global mut, not threadsafe.
    unsafe {
        if let Some(t) = &mut TRACE {
            return t.close();
        }
    }
    Ok(())
}

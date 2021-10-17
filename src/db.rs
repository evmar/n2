//! The n2 database stores information about previous builds for determining which files are up
//! to date.

use crate::graph::BuildId;
use crate::graph::FileId;
use crate::graph::Graph;
use crate::load::Loader;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;

/// Files are represented as integers that are stable across n2 executions.
#[derive(Debug, Clone, Copy)]
pub struct Id(usize);

/// The loaded state of a database, as needed to make updates to the stored
/// state.  Other state is directly loaded into the build graph.
pub struct State {
    /// Maps db::Id to FileId.
    fileids: Vec<FileId>,
    /// Maps FileId to db::Id.
    db_ids: HashMap<FileId, Id>,
}
impl State {
    pub fn new() -> Self {
        State {
            fileids: Vec::new(),
            db_ids: HashMap::new(),
        }
    }
}

/// An opened database, ready for writes.
pub struct Writer {
    state: State,
    w: BufWriter<File>,
}

impl Writer {
    fn new(state: State, w: File) -> Self {
        Writer {
            state: state,
            w: BufWriter::new(w),
        }
    }
    fn write_file(&mut self, name: &str) -> std::io::Result<()> {
        if name.len() >= 0b1000_0000 {
            panic!("filename too long");
        }
        let len = name.len() as u16;
        if len == 0 {
            panic!("no name");
        }
        self.w.write_all(&[(len & 0xFF) as u8, (len >> 8) as u8])?;
        self.w.write_all(name.as_bytes())?;
        self.w.flush()
    }

    fn ensure_id(&mut self, graph: &Graph, fileid: FileId) -> std::io::Result<Id> {
        let id = match self.state.db_ids.get(&fileid) {
            Some(&id) => id,
            None => {
                let id = Id(self.state.fileids.len());
                self.state.db_ids.insert(fileid, id);
                self.write_file(&graph.file(fileid).name)?;
                id
            }
        };
        Ok(id)
    }

    pub fn write_state(&mut self, graph: &Graph, id: BuildId) -> std::io::Result<()> {
        let build = graph.build(id);
        for &id in &build.outs {
            self.ensure_id(graph, id)?;
        }
        // TODO write build
        Ok(())
    }
}

/// Provides lower-level methods for reading serialized data.
struct BReader<'a> {
    r: BufReader<&'a mut File>,
}
#[allow(deprecated)] // don't care about your fancy uninit API
impl<'a> BReader<'a> {
    fn read_u16(&mut self) -> std::io::Result<u16> {
        let mut buf: [u8; 2];
        unsafe {
            buf = std::mem::uninitialized();
            self.r.read_exact(&mut buf)?;
        }
        Ok(((buf[1] as u16) << 8) | (buf[0] as u16))
    }
    fn read_str(&mut self, len: usize) -> std::io::Result<String> {
        // TODO: use uninit memory here
        let mut buf = Vec::new();
        buf.resize(len as usize, 0);
        self.r.read(buf.as_mut_slice())?;
        Ok(unsafe { String::from_utf8_unchecked(buf) })
    }
}

fn read(loader: &mut Loader, mut f: File) -> Result<Writer, String> {
    let mut r = BReader {
        r: std::io::BufReader::new(&mut f),
    };
    let mut state = State::new();

    loop {
        let len = match r.read_u16() {
            Ok(r) => r,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err.to_string()),
        };
        if len & 0b1000_0000_0000_0000 == 0 {
            let name = r.read_str(len as usize).map_err(|err| err.to_string())?;
            let fileid = loader.file_id(name);
            state.db_ids.insert(fileid, Id(state.fileids.len()));
            state.fileids.push(fileid);
        } else {
            // TODO: deps
        }
    }

    Ok(Writer::new(state, f))
}

/// Opens an on-disk database, loading its state into the provided Loader.
pub fn open(loader: &mut Loader, path: &str) -> Result<Writer, String> {
    match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
    {
        Ok(f) => Ok(read(loader, f)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let f =
                std::fs::File::create(path).map_err(|err| format!("create {}: {}", path, err))?;
            Ok(Writer::new(State::new(), f))
        }
        Err(err) => Err(err.to_string()),
    }
}

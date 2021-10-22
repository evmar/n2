//! The n2 database stores information about previous builds for determining which files are up
//! to date.

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

fn write_id(w: &mut BufWriter<File>, id: Id) -> std::io::Result<()> {
    let n = id.0 as u32;
    if n > (1 << 24) {
        panic!("too many fileids");
    }
    w.write_all(&[(n >> 16) as u8, (n >> 8) as u8, n as u8])
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
        self.w.write_all(&[(len >> 8) as u8, (len & 0xFF) as u8])?;
        self.w.write_all(name.as_bytes())?;
        self.w.flush()
    }

    fn ensure_id(&mut self, graph: &Graph, fileid: FileId) -> std::io::Result<Id> {
        let id = match self.state.db_ids.get(&fileid) {
            Some(&id) => id,
            None => {
                let id = Id(self.state.fileids.len());
                self.state.db_ids.insert(fileid, id);
                self.state.fileids.push(fileid);
                self.write_file(&graph.file(fileid).name)?;
                id
            }
        };
        Ok(id)
    }

    pub fn write_deps(
        &mut self,
        graph: &Graph,
        outs: &[FileId],
        deps: &[FileId],
    ) -> std::io::Result<()> {
        let mut dbdeps = Vec::new();
        for &dep in deps {
            let id = self.ensure_id(graph, dep)?;
            dbdeps.push(id);
        }
        let mark: u16 = dbdeps.len() as u16;
        println!("deps {:?}", dbdeps);
        for &out in outs {
            let id = self.ensure_id(graph, out)?;
            self.w
                .write_all(&[((mark >> 8) as u8) | 0b1000_0000, (mark & 0xFF) as u8])?;
            write_id(&mut self.w, id)?;
            for &dep in &dbdeps {
                write_id(&mut self.w, dep)?;
            }
        }
        self.w.flush()
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
        Ok(((buf[0] as u16) << 8) | (buf[1] as u16))
    }
    fn read_u24(&mut self)  -> std::io::Result<u32> {
        let mut buf: [u8; 3];
        unsafe {
            buf = std::mem::uninitialized();
            self.r.read_exact(&mut buf)?;
        }
        Ok(((buf[0] as u32) << 16) | ((buf[1] as u32) << 8)| (buf[2] as u32))
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
        let mut len = match r.read_u16() {
            Ok(r) => r,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err.to_string()),
        };
        let mask = 0b1000_0000_0000_0000;
        if len & mask  == 0 {
            let name = r.read_str(len as usize).map_err(|err| err.to_string())?;
            let fileid = loader.graph.file_id(&name);
            state.db_ids.insert(fileid, Id(state.fileids.len()));
            state.fileids.push(fileid);
        } else {
            len = len & !mask;
            let out = r.read_u24().map_err(|err| err.to_string())?;
            let mut ins = Vec::new();
            for _ in 0..len {
                ins.push(r.read_u24().map_err(|err| err.to_string())?);
            }
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

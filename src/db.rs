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

#[derive(Debug, Clone, Copy)]
pub struct Id(usize);

pub struct State {
    // Maps db::Id to FileId.
    fileids: Vec<FileId>,
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

pub struct Writer {
    state: State,
    w: BufWriter<File>,
}

impl Writer {
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

type Result<T> = std::result::Result<T, String>;
fn str_from_io(err: std::io::Error) -> String {
    format!("{}", err)
}

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
        let mut buf = Vec::new();
        buf.resize(len as usize, 0);
        self.r.read(buf.as_mut_slice())?;
        Ok(unsafe { String::from_utf8_unchecked(buf) })
    }
}

struct DBRead {
    file: File,
    state: State,
}

fn read(loader: &mut Loader, mut f: File) -> Result<DBRead> {
    let mut r = BReader {
        r: std::io::BufReader::new(&mut f),
    };
    let mut state = State::new();

    loop {
        let len = match r.read_u16() {
            Ok(r) => r,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::UnexpectedEof {
                    break;
                }
                return Err(str_from_io(err));
            }
        };
        if len & 0b1000_0000_0000_0000 == 0 {
            let name = r.read_str(len as usize).map_err(str_from_io)?;
            let fileid = loader.file_id(name);
            state.db_ids.insert(fileid, Id(state.fileids.len()));
            state.fileids.push(fileid);
        } else {
            // TODO: deps
        }
    }

    let db = DBRead {
        file: f,
        state: state,
    };
    Ok(db)
}

pub fn open(loader: &mut Loader, path: &str) -> Result<Writer> {
    let prev = match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
    {
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                let f = std::fs::File::create(path)
                    .map_err(|err| format!("create {}: {}", path, err))?;
                DBRead {
                    file: f,
                    state: State::new(),
                }
            } else {
                return Err(str_from_io(err));
            }
        }
        Ok(f) => read(loader, f)?,
    };
    Ok(Writer {
        state: prev.state,
        w: BufWriter::new(prev.file),
    })
}


use std::hash::Hasher;

struct Hash(u64);

struct FileId(usize);
struct File {
  name: String,
  mtime: u64,
}

struct Command {
  ins: Vec<FileId>,
  outs: Vec<FileId>,
}

impl Command {
  fn cmdline(&self) -> String {
    String::new()
  }
  fn hash(&self, g: &Graph) -> Hash {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for id in &self.ins {
      let inf = &g.files[id.0];
      h.write(inf.name.as_bytes());
      h.write_u64(inf.mtime);
      h.write_u8('\n' as u8);
    }
    h.write(self.cmdline().as_bytes());
    Hash(h.finish())
  }
}

struct Graph {
  files: Vec<File>,
  command: Vec<Command>,
}



/**
 * DataNodes represent computed state
 * XNodes transform DataNodes
 * 
 *   XNode(compute mtime of foo) ->
 *   DataNode(mtime(foo)) ->
 *   XNode(build bar) ->
 *   DataNode(mtime(bar))
*/

struct Hash(u64);

struct DNodeId(u32);
struct DNode {

}

struct XNodeId(u32);
struct XNodeX {
    ins: Vec<DNodeId>,
    outs: Vec<DNodeId>,
    check: i8,
    run: i8,
}

trait XNode {
    fn state(&self) -> Hash;
    fn run(&self);
}

struct MTime(std::num::NonZeroU64);
impl MTime {
    fn new() -> MTime {
        MTime(unsafe { std::num::NonZeroU64::new_unchecked(1) })
    }
}

struct DMTime {
    mtime: Option<MTime>,
}

struct XMTime {
    out: *mut DMTime,
}
impl XNode for XMTime {
    fn state(&self) -> Hash {
        Hash(0)
    }
    fn run(&self) {
        let dm = unsafe { &mut *self.out };
        dm.mtime = Some(MTime::new());
        // stat the file?  on another thread?
    }
}

struct XCmd {
}
impl XCmd {
    fn ins<F: Fn(Hash)>(f: F) {
        f(Hash(0));
        f(Hash(1));
    }
}
impl XNode for XCmd {
    fn state(&self) -> Hash {
        // hash all incoming mtimes
        Hash(0)
    }
    fn run(&self) {
        // execute the subcommand
    }
}

struct Graph {
    dnodes: Vec<u32>,
    xnodes: Vec<u32>,
}

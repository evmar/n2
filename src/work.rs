use crate::graph::{BuildId, FileId, Graph, State};
use std::collections::HashSet;

#[derive(Debug)]
pub struct Work {
    want: HashSet<BuildId>,
    ready: HashSet<BuildId>,
}

impl Work {
    pub fn new() -> Self {
        Work {
            want: HashSet::new(),
            ready: HashSet::new(),
        }
    }

    fn want_build(
        &mut self,
        graph: &Graph,
        state: &mut State,
        last_state: &State,
        id: BuildId,
    ) -> std::io::Result<bool> {
        if self.want.contains(&id) || self.ready.contains(&id) {
            return Ok(true);
        }

        // Visit inputs first, to discover if any are out of date.
        let mut input_dirty = false;
        for &id in &graph.build(id).ins {
            let d = self.want_file(graph, state, last_state, id)?;
            input_dirty = input_dirty || d;
        }

        let mut dirty = input_dirty;
        if !dirty {
            dirty = match last_state.get_hash(id) {
                None => true,
                Some(hash) => hash != state.hash(graph, id)?,
            };
        }

        if dirty {
            self.want.insert(id);
            if !input_dirty {
                self.ready.insert(id);
            }
        }

        return Ok(dirty);
    }

    pub fn want_file(
        &mut self,
        graph: &Graph,
        state: &mut State,
        last_state: &State,
        id: FileId,
    ) -> std::io::Result<bool> {
        match graph.file(id).input {
            None => Ok(false),
            Some(id) => self.want_build(graph, state, last_state, id),
        }
    }
}

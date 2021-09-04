use crate::graph::FileId;
use crate::parse::{NString, Statement};
use crate::{graph, parse};
use std::collections::HashMap;

pub fn read() -> Result<(), String> {
    let mut bytes = match std::fs::read("build.ninja") {
        Ok(b) => b,
        Err(e) => return Err(format!("read build.ninja: {}", e)),
    };
    bytes.push(0);

    let mut p = parse::Parser::new(&bytes);

    let mut graph = graph::Graph::new();
    let mut file_to_id: HashMap<NString, FileId> = HashMap::new();
    fn file_id(
        graph: &mut graph::Graph,
        hash: &mut HashMap<NString, FileId>,
        f: NString,
    ) -> FileId {
        // TODO: so many string copies :<
        match hash.get(&f) {
            Some(id) => *id,
            None => {
                let id = graph.add_file(f.clone());
                hash.insert(f, id.clone());
                id
            }
        }
    }

    let mut default: Option<FileId> = None;
    loop {
        let stmt = match p.read().map_err(|err| p.format_parse_error(err))? {
            None => break,
            Some(s) => s,
        };
        match stmt {
            Statement::Default(f) => match file_to_id.get(&f.to_nstring()) {
                Some(id) => default = Some(*id),
                None => return Err(format!("unknown default {:?}", f)),
            },
            Statement::Rule(r) => println!("TODO {:?}", r),
            Statement::Build(b) => {
                let ins: Vec<FileId> = b
                    .ins
                    .into_iter()
                    .map(|f| file_id(&mut graph, &mut file_to_id, f))
                    .collect();
                let outs: Vec<FileId> = b
                    .outs
                    .into_iter()
                    .map(|f| file_id(&mut graph, &mut file_to_id, f))
                    .collect();
                graph.add_build(graph::Build {
                    ins: ins,
                    outs: outs,
                });
            }
        };
    }
    println!("files {:?}", file_to_id);
    println!("default {:?}", default);
    Ok(())
}

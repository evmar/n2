use hashbrown::raw::RawTable;
use std::hash::Hasher;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Symbol(usize);

struct EndTab {
    data: Vec<u8>,
    ends: Vec<usize>,
}

impl EndTab {
    fn add(&mut self, s: &[u8]) -> Symbol {
        self.data.extend_from_slice(s);
        let sym = Symbol(self.ends.len());
        self.ends.push(self.data.len());
        sym
    }
    fn get(&self, sym: Symbol) -> &[u8] {
        let start = if sym.0 > 0 { self.ends[sym.0 - 1] } else { 0 };
        let end = self.ends[sym.0];
        &self.data[start..end]
    }
}

pub struct Intern {
    lookup: RawTable<Symbol>,
    endtab: EndTab,
}

fn hash_bytes(s: &[u8]) -> u64 {
    let mut hasher = ahash::AHasher::default();
    hasher.write(s);
    hasher.finish()
}

impl Intern {
    pub fn new() -> Intern {
        Intern {
            lookup: RawTable::new(),
            endtab: EndTab{
                data: Vec::new(),
                ends: Vec::new(),
            }
        }
    }

    pub fn add<'a>(&mut self, s: &'a [u8]) -> Symbol {
        let hash = hash_bytes(s);
        if let Some(sym) = self
            .lookup
            .get(hash, |sym: &Symbol| s == self.endtab.get(*sym))
        {
            return *sym;
        }
        let sym = self.endtab.add(s);

        let endtab = &self.endtab;
        self.lookup
            .insert(hash, sym, |sym: &Symbol| hash_bytes(endtab.get(*sym)));
        sym
    }

    pub fn get(&self, sym: Symbol) -> &[u8] {
        self.endtab.get(sym)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user() {
        let mut i = Intern::new();
        let hi = i.add("hi".as_bytes());
        let yo = i.add("yo".as_bytes());
        let hi2 = i.add("hi".as_bytes());
        assert_eq!(hi, hi2);
        assert_ne!(hi, yo);

        assert_eq!(i.get(hi), "hi".as_bytes());
        assert_eq!(i.get(yo), "yo".as_bytes());
    }
}

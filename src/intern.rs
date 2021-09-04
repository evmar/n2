use hashbrown::raw::RawTable;
use std::hash::Hasher;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Symbol(usize);

struct EndTab {
    strs: String,
    ends: Vec<usize>,
}

impl EndTab {
    fn add(&mut self, s: &str) -> Symbol {
        self.strs.push_str(s);
        let sym = Symbol(self.ends.len());
        self.ends.push(self.strs.len());
        sym
    }
    fn get(&self, sym: Symbol) -> &str {
        let start = if sym.0 > 0 { self.ends[sym.0 - 1] } else { 0 };
        let end = self.ends[sym.0];
        &self.strs[start..end]
    }
}

pub struct Intern {
    lookup: RawTable<Symbol>,
    endtab: EndTab,
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = ahash::AHasher::default();
    hasher.write(s.as_bytes());
    hasher.finish()
}

impl Intern {
    pub fn new() -> Intern {
        Intern {
            lookup: RawTable::new(),
            endtab: EndTab{
                strs: String::new(),
                ends: Vec::new(),
            }
        }
    }

    pub fn add<'a>(&mut self, s: &'a str) -> Symbol {
        let hash = hash_str(s);
        if let Some(sym) = self
            .lookup
            .get(hash, |sym: &Symbol| s == self.endtab.get(*sym))
        {
            return *sym;
        }
        let sym = self.endtab.add(s);

        let endtab = &self.endtab;
        self.lookup
            .insert(hash, sym, |sym: &Symbol| hash_str(endtab.get(*sym)));
        sym
    }

    pub fn get(&self, sym: Symbol) -> &str {
        self.endtab.get(sym)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user() {
        let mut i = Intern::new();
        let hi = i.add("hi");
        let yo = i.add("yo");
        let hi2 = i.add("hi");
        assert_eq!(hi, hi2);
        assert_ne!(hi, yo);

        assert_eq!(i.get(hi), "hi");
        assert_eq!(i.get(yo), "yo");
    }
}

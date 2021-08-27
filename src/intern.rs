use std::marker::PhantomData;
pub struct Intern {
    spans: Vec<(usize, usize)>,
    strings: String,
    //_p: PhantomData<& 'a ()>,
}
pub struct Id<'a>(usize, PhantomData<&'a ()>);

impl Intern {
    fn get<'id, 'a: 'id>(&'a self, _s: &str) -> Id<'id> {
        Id(0, PhantomData)
    }
    fn str<'id, 'a: 'id>(&'a mut self, id: &Id<'id>) -> &'a str {
        let (start, end) = self.spans[id.0];
        &self.strings[start..end]
    }
}

pub fn user() -> &'static str {
    let mut i2 = Intern{spans:Vec::new(), strings:String::new()};
    let q = {
        let k = i2.get("foo");
        i2.str(&k)
    };
    println!("{}", q);
    let k2 = i2.get("foo2");
    q
}
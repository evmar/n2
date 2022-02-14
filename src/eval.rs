//! Represents parsed Ninja strings with embedded variable references, e.g.
//! `c++ $in -o $out`, and mechanisms for expanding those into plain strings.

use std::collections::HashMap;

/// An environment providing a mapping of variable name to variable value.
/// A given EvalString may be expanded with multiple environments as possible
/// context.
pub trait Env {
    fn get_var(&self, var: &str) -> Option<String>;
}

/// One token within an EvalString, either literal text or a variable reference.
#[derive(Debug)]
pub enum EvalPart<T: AsRef<str>> {
    Literal(T),
    VarRef(T),
}

/// A parsed but unexpanded variable-reference string, e.g. "cc $in -o $out".
#[derive(Debug)]
pub struct EvalString<T: AsRef<str>>(Vec<EvalPart<T>>);
impl<T: AsRef<str>> EvalString<T> {
    pub fn new(parts: Vec<EvalPart<T>>) -> Self {
        EvalString(parts)
    }
    pub fn evaluate(&self, envs: &[&dyn Env]) -> String {
        let mut val = String::new();
        for part in &self.0 {
            match part {
                EvalPart::Literal(s) => val.push_str(s.as_ref()),
                EvalPart::VarRef(v) => {
                    for env in envs {
                        if let Some(v) = env.get_var(v.as_ref()) {
                            val.push_str(&v);
                            break;
                        }
                    }
                }
            }
        }
        val
    }
}
impl EvalString<&str> {
    pub fn into_owned(self) -> EvalString<String> {
        EvalString(
            self.0
                .into_iter()
                .map(|part| match part {
                    EvalPart::Literal(s) => EvalPart::Literal(s.to_owned()),
                    EvalPart::VarRef(s) => EvalPart::VarRef(s.to_owned()),
                })
                .collect(),
        )
    }
}

/// A single scope's worth of variable definitions.
#[derive(Debug)]
pub struct Vars<'a>(HashMap<&'a str, String>);
#[allow(clippy::new_without_default)]
impl<'a> Vars<'a> {
    pub fn new() -> Vars<'a> {
        Vars(HashMap::new())
    }
    pub fn insert(&mut self, key: &'a str, val: String) {
        self.0.insert(key, val);
    }
    pub fn get(&self, key: &'a str) -> Option<&String> {
        self.0.get(key)
    }
}
impl<'a> Env for Vars<'a> {
    fn get_var(&self, var: &str) -> Option<String> {
        self.0.get(var).cloned()
    }
}

/// A single scope's worth of variable definitions, before $-expansion.
/// For variables attached to a rule we keep them unexpanded in memory because
/// they may be expanded in multiple different ways depending on which rule uses
/// them.
#[derive(Debug)]
pub struct LazyVars(Vec<(String, EvalString<String>)>);
#[allow(clippy::new_without_default)]
impl LazyVars {
    pub fn new() -> Self {
        LazyVars(Vec::new())
    }
    pub fn insert(&mut self, key: String, val: EvalString<String>) {
        self.0.push((key, val));
    }
    pub fn get(&self, key: &str) -> Option<&EvalString<String>> {
        for (k, v) in &self.0 {
            if k == key {
                return Some(v);
            }
        }
        None
    }
    pub fn keyvals(&self) -> &Vec<(String, EvalString<String>)> {
        &self.0
    }
}
impl Env for LazyVars {
    fn get_var(&self, var: &str) -> Option<String> {
        self.get(var).map(|val| val.evaluate(&[]))
    }
}

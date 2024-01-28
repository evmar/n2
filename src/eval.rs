//! Represents parsed Ninja strings with embedded variable references, e.g.
//! `c++ $in -o $out`, and mechanisms for expanding those into plain strings.

use crate::load::Scope;
use crate::load::ScopePosition;
use crate::smallmap::SmallMap;
use std::borrow::Borrow;
use std::borrow::Cow;

/// An environment providing a mapping of variable name to variable value.
/// This represents one "frame" of evaluation context, a given EvalString may
/// need multiple environments in order to be fully expanded.
pub trait Env {
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<str>>>;
}

/// One token within an EvalString, either literal text or a variable reference.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalPart<T: AsRef<str>> {
    Literal(T),
    VarRef(T),
}

/// A parsed but unexpanded variable-reference string, e.g. "cc $in -o $out".
/// This is generic to support EvalString<&str>, which is used for immediately-
/// expanded evals, like top-level bindings, and EvalString<String>, which is
/// used for delayed evals like in `rule` blocks.
#[derive(Debug, PartialEq)]
pub struct EvalString<T: AsRef<str>>(Vec<EvalPart<T>>);
impl<T: AsRef<str>> EvalString<T> {
    pub fn new(parts: Vec<EvalPart<T>>) -> Self {
        EvalString(parts)
    }

    fn evaluate_inner(
        &self,
        result: &mut String,
        envs: &[&dyn Env],
        scope: &Scope,
        position: ScopePosition,
    ) {
        for part in &self.0 {
            match part {
                EvalPart::Literal(s) => result.push_str(s.as_ref()),
                EvalPart::VarRef(v) => {
                    let mut found = false;
                    for (i, env) in envs.iter().enumerate() {
                        if let Some(v) = env.get_var(v.as_ref()) {
                            v.evaluate_inner(result, &envs[i + 1..], scope, position);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        scope.evaluate(result, v.as_ref(), position);
                    }
                }
            }
        }
    }

    /// evalulate turns the EvalString into a regular String, looking up the
    /// values of variable references in the provided Envs. It will look up
    /// its variables in the earliest Env that has them, and then those lookups
    /// will be recursively expanded starting from the env after the one that
    /// had the first successful lookup.
    pub fn evaluate(&self, envs: &[&dyn Env], scope: &Scope, position: ScopePosition) -> String {
        let mut result = String::new();
        self.evaluate_inner(&mut result, envs, scope, position);
        result
    }

    pub fn maybe_literal(&self) -> Option<&T> {
        match &self.0[..] {
            [EvalPart::Literal(x)] => Some(x),
            _ => None,
        }
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

impl EvalString<String> {
    pub fn as_cow(&self) -> EvalString<Cow<str>> {
        EvalString(
            self.0
                .iter()
                .map(|part| match part {
                    EvalPart::Literal(s) => EvalPart::Literal(Cow::Borrowed(s.as_ref())),
                    EvalPart::VarRef(s) => EvalPart::VarRef(Cow::Borrowed(s.as_ref())),
                })
                .collect(),
        )
    }
}

impl EvalString<&str> {
    pub fn as_cow(&self) -> EvalString<Cow<str>> {
        EvalString(
            self.0
                .iter()
                .map(|part| match part {
                    EvalPart::Literal(s) => EvalPart::Literal(Cow::Borrowed(*s)),
                    EvalPart::VarRef(s) => EvalPart::VarRef(Cow::Borrowed(*s)),
                })
                .collect(),
        )
    }
}

impl<K: Borrow<str> + PartialEq> Env for SmallMap<K, EvalString<String>> {
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<str>>> {
        Some(self.get(var)?.as_cow())
    }
}

impl<K: Borrow<str> + PartialEq> Env for SmallMap<K, EvalString<&str>> {
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<str>>> {
        Some(self.get(var)?.as_cow())
    }
}

impl Env for SmallMap<&str, String> {
    fn get_var(&self, var: &str) -> Option<EvalString<Cow<str>>> {
        Some(EvalString::new(vec![EvalPart::Literal(
            std::borrow::Cow::Borrowed(self.get(var)?),
        )]))
    }
}

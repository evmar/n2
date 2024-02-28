//! Represents parsed Ninja strings with embedded variable references, e.g.
//! `c++ $in -o $out`, and mechanisms for expanding those into plain strings.

use crate::load::Scope;
use crate::load::ScopePosition;
use crate::parse::EvalParser;
use crate::smallmap::SmallMap;
use std::borrow::Borrow;

/// An environment providing a mapping of variable name to variable value.
/// This represents one "frame" of evaluation context, a given EvalString may
/// need multiple environments in order to be fully expanded.
pub trait Env {
    fn evaluate_var(
        &self,
        result: &mut String,
        var: &str,
        envs: &[&dyn Env],
        scope: &Scope,
        position: ScopePosition,
    );
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
pub struct EvalString<T: AsRef<str>>(T);
impl<T: AsRef<str>> EvalString<T> {
    pub fn new(inner: T) -> Self {
        EvalString(inner)
    }

    pub fn evaluate_inner(
        &self,
        result: &mut String,
        envs: &[&dyn Env],
        scope: &Scope,
        position: ScopePosition,
    ) {
        for part in self.parse() {
            match part {
                EvalPart::Literal(s) => result.push_str(s.as_ref()),
                EvalPart::VarRef(v) => {
                    if let Some(env) = envs.first() {
                        env.evaluate_var(result, v.as_ref(), &envs[1..], scope, position);
                    } else {
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
        if self.0.as_ref().contains('$') {
            None
        } else {
            Some(&self.0)
        }
    }

    pub fn parse(&self) -> impl Iterator<Item = EvalPart<&str>> {
        EvalParser::new(self.0.as_ref().as_bytes())
    }
}

impl EvalString<&str> {
    pub fn into_owned(self) -> EvalString<String> {
        EvalString(self.0.to_owned())
    }
}

impl<K: Borrow<str> + PartialEq, V: AsRef<str>> Env for SmallMap<K, EvalString<V>> {
    fn evaluate_var(
        &self,
        result: &mut String,
        var: &str,
        envs: &[&dyn Env],
        scope: &Scope,
        position: ScopePosition,
    ) {
        if let Some(v) = self.get(var) {
            v.evaluate_inner(result, envs, scope, position);
        } else if let Some(env) = envs.first() {
            env.evaluate_var(result, var, &envs[1..], scope, position);
        } else {
            scope.evaluate(result, var, position);
        }
    }
}

use crate::Filter;
use serde_json::Value;
use std::rc::Rc;

#[derive(Debug, Clone, Default)]
pub(crate) struct Environment(Option<Rc<Binding>>);

#[derive(Debug)]
struct Binding {
    name: String,
    value: Value,
    parent: Environment,
}

impl Environment {
    pub(crate) fn bind(&self, name: &str, value: Value) -> Self {
        Self(Some(Rc::new(Binding {
            name: name.to_owned(),
            value,
            parent: self.clone(),
        })))
    }

    pub(crate) fn get(&self, name: &str) -> Option<Value> {
        let mut environment = self;
        while let Some(binding) = &environment.0 {
            if binding.name == name {
                return Some(binding.value.clone());
            }
            environment = &binding.parent;
        }
        None
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionEnvironment(Option<Rc<FunctionBinding>>);

#[derive(Debug, Clone)]
pub(crate) struct FunctionBinding {
    pub(crate) parameters: Vec<String>,
    pub(crate) body: Filter,
    name: String,
    parent: FunctionEnvironment,
}

impl FunctionEnvironment {
    pub(crate) fn bind(&self, name: &str, parameters: &[String], body: &Filter) -> Self {
        Self(Some(Rc::new(FunctionBinding {
            name: name.to_owned(),
            parameters: parameters.to_vec(),
            body: body.clone(),
            parent: self.clone(),
        })))
    }

    pub(crate) fn get(&self, name: &str) -> Option<Rc<FunctionBinding>> {
        let mut environment = self;
        while let Some(binding) = &environment.0 {
            if binding.name == name {
                return Some(Rc::clone(binding));
            }
            environment = &binding.parent;
        }
        None
    }
}

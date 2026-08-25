use std::{collections::BTreeMap, ffi::OsString};

use crate::node::{
    cli::Prompter,
    command::Runner,
    platform::{HomePaths, value},
};

pub struct AppContext<'a> {
    pub runner: &'a dyn Runner,
    pub prompt: &'a mut dyn Prompter,
    pub home: HomePaths,
    pub environment: BTreeMap<OsString, OsString>,
}

impl AppContext<'_> {
    pub fn env(&self, key: &str) -> Option<String> {
        value(&self.environment, key)
    }
}

use std::cell::RefCell;

use crate::node::error::{AppError, Result};

pub trait Prompter {
    fn confirm(&mut self, question: &str) -> Result<bool>;

    fn intro(&self, _title: &str) -> Result<()> {
        Ok(())
    }

    fn note(&self, _title: &str, _message: &str) -> Result<()> {
        Ok(())
    }

    fn cancel(&self, _message: &str) -> Result<()> {
        Ok(())
    }

    fn start_progress(&self, _message: &str) -> Result<()> {
        Ok(())
    }

    fn finish_progress(&self, _message: &str) -> Result<()> {
        Ok(())
    }

    fn fail_progress(&self, _message: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
pub struct TerminalPrompter {
    progress: RefCell<Option<cliclack::ProgressBar>>,
}

impl Prompter for TerminalPrompter {
    fn confirm(&mut self, question: &str) -> Result<bool> {
        cliclack::confirm(question)
            .initial_value(false)
            .interact()
            .map_err(|error| AppError::io("read confirmation", None, error))
    }

    fn intro(&self, title: &str) -> Result<()> {
        cliclack::intro(title).map_err(|error| AppError::io("render intro", None, error))
    }

    fn note(&self, title: &str, message: &str) -> Result<()> {
        cliclack::note(title, message).map_err(|error| AppError::io("render note", None, error))
    }

    fn cancel(&self, message: &str) -> Result<()> {
        cliclack::outro_cancel(message)
            .map_err(|error| AppError::io("render cancellation", None, error))
    }

    fn start_progress(&self, message: &str) -> Result<()> {
        let progress = cliclack::spinner();
        progress.start(message);
        self.progress.replace(Some(progress));
        Ok(())
    }

    fn finish_progress(&self, message: &str) -> Result<()> {
        if let Some(progress) = self.progress.borrow_mut().take() {
            progress.stop(message);
        }
        Ok(())
    }

    fn fail_progress(&self, message: &str) -> Result<()> {
        if let Some(progress) = self.progress.borrow_mut().take() {
            progress.error(message);
        }
        Ok(())
    }
}

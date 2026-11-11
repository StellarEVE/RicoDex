//! Snapshot based undo and redo. Every committed state is a full project
//! clone, which keeps undo simple and exact: restoring a state restores
//! everything, with no inverse commands to get wrong.

use crate::command::{Command, CommandError};
use crate::model::Project;

/// A project plus its undo and redo stacks.
#[derive(Clone, Debug)]
pub struct History {
    current: Project,
    undo_stack: Vec<Project>,
    redo_stack: Vec<Project>,
}

impl History {
    pub fn new(project: Project) -> History {
        History {
            current: project,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn project(&self) -> &Project {
        &self.current
    }

    /// Apply a command. On success the previous state becomes undoable and
    /// the redo stack is cleared. On rejection nothing changes at all,
    /// including both stacks.
    pub fn apply(&mut self, command: &Command) -> Result<(), CommandError> {
        // Project::apply works on a clone and returns the original on
        // rejection, so feed it a clone and keep self.current in place
        // until the new state is known to be valid.
        match self.current.clone().apply(command) {
            Ok(next) => {
                let previous = std::mem::replace(&mut self.current, next);
                self.undo_stack.push(previous);
                self.redo_stack.clear();
                Ok(())
            }
            Err((_, error)) => Err(error),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Restore the previous state. Returns false when there is nothing to
    /// undo, leaving the current state in place.
    pub fn undo(&mut self) -> bool {
        match self.undo_stack.pop() {
            Some(previous) => {
                let current = std::mem::replace(&mut self.current, previous);
                self.redo_stack.push(current);
                true
            }
            None => false,
        }
    }

    /// Restore the next state after an undo. Returns false when there is
    /// nothing to redo.
    pub fn redo(&mut self) -> bool {
        match self.redo_stack.pop() {
            Some(next) => {
                let current = std::mem::replace(&mut self.current, next);
                self.undo_stack.push(current);
                true
            }
            None => false,
        }
    }
}

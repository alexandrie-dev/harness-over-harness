#![allow(dead_code)]
pub mod programs;
use hoh_commander::{Command, CommanderError, Output, ParseOptions};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Capture {
    pub out: Arc<Mutex<String>>,
    pub err: Arc<Mutex<String>>,
}
impl Capture {
    pub fn install(&self, command: &mut Command) {
        let out = self.out.clone();
        let err = self.err.clone();
        command
            .configure_output(Output::new(
                move |s| out.lock().unwrap().push_str(s),
                move |s| err.lock().unwrap().push_str(s),
            ))
            .exit_override();
    }
    pub fn install_recursive(&self, command: &mut Command) {
        self.install(command);
        for child in &mut command.commands {
            self.install_recursive(child);
        }
    }
}
pub fn program() -> Command {
    let mut p = Command::new();
    Capture::default().install(&mut p);
    p
}
pub fn failure(command: &mut Command, args: &[&str]) -> CommanderError {
    match command.parse(args, ParseOptions::USER) {
        Err(e) => e.commander().unwrap().clone(),
        Ok(_) => panic!("expected commander exit"),
    }
}

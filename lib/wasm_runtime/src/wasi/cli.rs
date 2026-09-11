use super::{state::*, wasi::cli::*};
use crate::context::Context;
use wasmtime::{Result, component::Resource};
#[derive(Debug)]
pub struct Exit(pub u8);
impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "guest exited with status {}", self.0)
    }
}
impl std::error::Error for Exit {}
impl environment::Host for Context {
    fn get_environment(&mut self) -> Result<Vec<(String, String)>> {
        Ok(self
            .options
            .environment
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect())
    }
    fn get_arguments(&mut self) -> Result<Vec<String>> {
        Ok(self.options.arguments.clone())
    }
    fn initial_cwd(&mut self) -> Result<Option<String>> {
        Ok(self
            .options
            .environment
            .iter()
            .find(|e| e.name == "PWD")
            .map(|e| e.value.clone()))
    }
}
impl exit::Host for Context {
    fn exit(&mut self, status: Result<(), ()>) -> Result<()> {
        Err(Exit(if status.is_ok() { 0 } else { 1 }).into())
    }
    fn exit_with_code(&mut self, code: u8) -> Result<()> {
        Err(Exit(code).into())
    }
}
impl stdin::Host for Context {
    fn get_stdin(&mut self) -> Result<Resource<Input>> {
        use crate::resources::{Kind, READ};
        let input = match self.resources.find("wasi:stdin") {
            Some(id) => {
                if let Ok(e) = self.resources.get(id, Kind::File, READ) {
                    Input::File(e.clone(), 0)
                } else {
                    Input::Socket(self.resources.get(id, Kind::Socket, READ)?.handle.clone())
                }
            }
            None => Input::Empty,
        };
        self.push(input)
    }
}
impl stdout::Host for Context {
    fn get_stdout(&mut self) -> Result<Resource<Output>> {
        self.stdout_resource("wasi:stdout")
    }
}
impl stderr::Host for Context {
    fn get_stderr(&mut self) -> Result<Resource<Output>> {
        self.stdout_resource("wasi:stderr")
    }
}
impl terminal_input::Host for Context {}
impl terminal_input::HostTerminalInput for Context {
    fn drop(&mut self, r: Resource<TerminalInput>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl terminal_output::Host for Context {}
impl terminal_output::HostTerminalOutput for Context {
    fn drop(&mut self, r: Resource<TerminalOutput>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl terminal_stdin::Host for Context {
    fn get_terminal_stdin(&mut self) -> Result<Option<Resource<TerminalInput>>> {
        if self.has_terminal("wasi:stdin", crate::resources::READ) {
            Ok(Some(self.push(TerminalInput)?))
        } else {
            Ok(None)
        }
    }
}
impl terminal_stdout::Host for Context {
    fn get_terminal_stdout(&mut self) -> Result<Option<Resource<TerminalOutput>>> {
        if self.has_terminal("wasi:stdout", crate::resources::WRITE) {
            Ok(Some(self.push(TerminalOutput)?))
        } else {
            Ok(None)
        }
    }
}
impl terminal_stderr::Host for Context {
    fn get_terminal_stderr(&mut self) -> Result<Option<Resource<TerminalOutput>>> {
        if self.has_terminal("wasi:stderr", crate::resources::WRITE) {
            Ok(Some(self.push(TerminalOutput)?))
        } else {
            Ok(None)
        }
    }
}

impl Context {
    fn has_terminal(&self, stream: &str, rights: u32) -> bool {
        use crate::resources::{Kind, READ};
        self.resources
            .find("bexos:tty/session")
            .is_some_and(|id| self.resources.get(id, Kind::Channel, READ).is_ok())
            && self
                .resources
                .find(stream)
                .is_some_and(|id| self.resources.get(id, Kind::Socket, rights).is_ok())
    }
    fn stdout_resource(&mut self, name: &str) -> Result<Resource<Output>> {
        use crate::resources::{Kind, WRITE};
        let output = if let Some(id) = self.resources.find(name) {
            if let Ok(entry) = self.resources.get(id, Kind::File, WRITE) {
                Output::file(entry.clone(), 0)
            } else if let Ok(entry) = self.resources.get(id, Kind::Socket, WRITE) {
                Output::socket(entry.handle.clone())
            } else {
                Output::closed()
            }
        } else if self.child_permit.is_some() {
            Output::closed()
        } else {
            Output::log()
        };
        self.push(output)
    }
}

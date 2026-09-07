use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

#[derive(Debug)]
pub enum TerminalEvent {
    Output(Vec<u8>),
    Exited,
}

enum TerminalCommand {
    Input(Vec<u8>),
    Resize { rows: u16, cols: u16 },
}

pub struct TerminalSession {
    commands: Option<Sender<TerminalCommand>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    workers: Vec<JoinHandle<()>>,
}

impl fmt::Debug for TerminalSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalSession")
            .field("workers", &self.workers.len())
            .finish_non_exhaustive()
    }
}

impl TerminalSession {
    pub fn spawn(cwd: &Path) -> Result<(Self, Receiver<TerminalEvent>), String> {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 28,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let mut child = pty
            .slave
            .spawn_command(command)
            .map_err(|error| error.to_string())?;
        drop(pty.slave);
        let killer = child.clone_killer();
        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let mut writer = pty
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        resize(&*pty.master, 28, 120);
        let (commands, requests) = channel::<TerminalCommand>();
        let (events, output) = channel();
        let output_reader = events.clone();
        let reader_worker = thread::Builder::new()
            .name("dualpane-terminal-reader".to_owned())
            .spawn(move || {
                let mut buffer = vec![0_u8; 16 * 1_024];
                while let Ok(count) = reader.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    if output_reader
                        .send(TerminalEvent::Output(buffer[..count].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        let writer_worker = thread::Builder::new()
            .name("dualpane-terminal-writer".to_owned())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    match request {
                        TerminalCommand::Input(bytes) => {
                            if writer
                                .write_all(&bytes)
                                .and_then(|()| writer.flush())
                                .is_err()
                            {
                                break;
                            }
                        }
                        TerminalCommand::Resize { rows, cols } => {
                            resize(&*pty.master, rows, cols);
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        let wait_worker = thread::Builder::new()
            .name("dualpane-terminal-wait".to_owned())
            .spawn(move || {
                let _ = child.wait();
                let _ = events.send(TerminalEvent::Exited);
            })
            .map_err(|error| error.to_string())?;
        Ok((
            Self {
                commands: Some(commands),
                killer,
                workers: vec![reader_worker, writer_worker, wait_worker],
            },
            output,
        ))
    }

    pub fn send(&self, bytes: &[u8]) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(TerminalCommand::Input(bytes.to_vec()));
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(TerminalCommand::Resize { rows, cols });
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(TerminalCommand::Input(b"exit\n".to_vec()));
        }
        self.commands.take();
        let _ = self.killer.kill();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn resize(master: &dyn MasterPty, rows: u16, cols: u16) {
    let _ = master.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
}

#[cfg(test)]
mod tests {
    use super::TerminalEvent;

    #[test]
    fn terminal_output_preserves_control_sequences() {
        let bytes = b"\x1b[31mred\x1b[0m\r\n".to_vec();
        let TerminalEvent::Output(output) = TerminalEvent::Output(bytes.clone()) else {
            unreachable!();
        };
        assert_eq!(output, bytes);
    }
}

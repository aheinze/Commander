use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
    shutdown: Sender<()>,
    workers: Vec<JoinHandle<()>>,
}

// Also cleans up if any of the worker threads fails to start. Spawn itself runs
// on the terminal startup worker, never on GTK's thread.
struct ChildCleanup(Option<Box<dyn portable_pty::Child + Send + Sync>>);

impl Drop for ChildCleanup {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            // Unlike clone_killer(), the owned child's kill escalates to a
            // forced termination after the Unix SIGHUP grace period.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.cwd(cwd);
        Self::spawn_command(command)
    }

    fn spawn_command(
        mut command: CommandBuilder,
    ) -> Result<(Self, Receiver<TerminalEvent>), String> {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 28,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;
        command.env("TERM", "xterm-256color");
        let mut child = ChildCleanup(Some(
            pty.slave
                .spawn_command(command)
                .map_err(|error| error.to_string())?,
        ));
        drop(pty.slave);
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
        let (shutdown, stopping) = channel();
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
                loop {
                    match child.0.as_mut().unwrap().try_wait() {
                        Ok(Some(_)) => {
                            child.0.take(); // Already reaped; never signal a reused PID.
                            break;
                        }
                        Err(_) => break,
                        Ok(None) => {}
                    }
                    match stopping.recv_timeout(Duration::from_millis(50)) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
                drop(child);
                let _ = events.send(TerminalEvent::Exited);
            })
            .map_err(|error| error.to_string())?;
        Ok((
            Self {
                commands: Some(commands),
                shutdown,
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
        let _ = self.shutdown.send(());
        self.commands.take();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
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
    use super::*;
    use std::time::Instant;

    #[test]
    fn terminal_output_preserves_control_sequences() {
        let bytes = b"\x1b[31mred\x1b[0m\r\n".to_vec();
        let TerminalEvent::Output(output) = TerminalEvent::Output(bytes.clone()) else {
            unreachable!();
        };
        assert_eq!(output, bytes);
    }
    #[test]
    fn shutdown_is_nonblocking_and_reaps_a_child_that_ignores_hup() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "trap '' HUP; printf READY; exec sleep 30"]);
        let (mut session, events) = TerminalSession::spawn_command(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("READY") {
            match events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                TerminalEvent::Output(bytes) => output.extend(bytes),
                TerminalEvent::Exited => panic!("child exited before becoming ready"),
            }
        }
        let workers = std::mem::take(&mut session.workers);
        let started = Instant::now();
        drop(session);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "closing a tab waited for its child"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while workers.iter().any(|worker| !worker.is_finished()) {
            assert!(
                Instant::now() < deadline,
                "terminal workers did not shut down"
            );
            thread::sleep(Duration::from_millis(10));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(
            events
                .try_iter()
                .any(|event| matches!(event, TerminalEvent::Exited))
        );
    }

    #[test]
    fn natural_exit_reports_completion_and_keeps_output() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "printf finished"]);
        let (session, events) = TerminalSession::spawn_command(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        let mut exited = false;
        while !exited || !String::from_utf8_lossy(&output).contains("finished") {
            match events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                TerminalEvent::Output(bytes) => output.extend(bytes),
                TerminalEvent::Exited => exited = true,
            }
        }
        drop(session);
    }
}

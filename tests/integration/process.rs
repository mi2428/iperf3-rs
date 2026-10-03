#![allow(dead_code)]

use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

pub const BUILD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
pub const RUN_TIMEOUT: Duration = Duration::from_secs(60);
pub const READY_TIMEOUT: Duration = Duration::from_secs(15);
pub const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<io::Result<()>>,
    finished: bool,
}

impl Capture {
    fn drain(mut pipe: impl Read + Send + 'static) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let data = Arc::clone(&bytes);
        let (tx, done) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| {
                let mut chunk = [0_u8; 8192];
                loop {
                    let n = pipe.read(&mut chunk)?;
                    if n == 0 {
                        return Ok(());
                    }
                    let mut bytes = data.lock().unwrap();
                    // ponytail: 8 MiB per pipe; use a bounded file spool if build logs need more.
                    if bytes.len() + n > OUTPUT_LIMIT {
                        return Err(io::Error::other(
                            "test command output exceeds 8 MiB per pipe",
                        ));
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
            })();
            let _ = tx.send(result);
        });
        Self {
            bytes,
            done,
            finished: false,
        }
    }

    fn poll(&mut self) -> io::Result<()> {
        if !self.finished {
            match self.done.try_recv() {
                Ok(result) => {
                    result?;
                    self.finished = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::other(
                        "test command pipe reader exited unexpectedly",
                    ));
                }
            }
        }
        Ok(())
    }

    fn tail(&self) -> String {
        let bytes = self.bytes.lock().unwrap();
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]).into_owned()
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.bytes.lock().unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Pinned,
    Signaled,
    Reaped,
    Lost,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    KeeperStarted,
    GuardReady,
    CommandSpawn,
    GroupSignal,
    CommandWait,
    CommandReap,
    KeeperWait,
    KeeperReap,
}

#[cfg(test)]
thread_local! {
    static SPAWN_OBSERVER: std::cell::RefCell<Option<Arc<Mutex<Vec<Lifecycle>>>>> = const { std::cell::RefCell::new(None) };
    static PANIC_READERS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static WAIT_ERROR: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static SIGNAL_ERROR: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

pub struct OwnedCommand {
    child: Option<Child>,
    keeper: Option<Child>,
    label: String,
    deadline: Instant,
    timeout: Duration,
    stdout: Option<Capture>,
    stderr: Option<Capture>,
    ownership: Ownership,
    #[cfg(test)]
    lifecycle: Arc<Mutex<Vec<Lifecycle>>>,
}

impl OwnedCommand {
    pub fn spawn(command: &mut Command, timeout: Duration) -> io::Result<Self> {
        Self::spawn_with_output(command, timeout, true)
    }

    fn spawn_with_output(
        command: &mut Command,
        timeout: Duration,
        capture: bool,
    ) -> io::Result<Self> {
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "test command deadline is out of range",
            )
        })?;
        let label = format!(
            "{:?} {:?}",
            command.get_program(),
            command.get_args().collect::<Vec<_>>()
        );
        #[cfg(test)]
        let lifecycle = SPAWN_OBSERVER
            .with(|observer| observer.borrow().clone())
            .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())));
        // ponytail: one extra owned process per command; a native guardian only
        // if this bounded test-runner overhead becomes material.
        let keeper = Command::new("/bin/sh")
            .args(["-c", "read -r _"])
            .env_clear()
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // This guard exists before actual exec/reader startup. The keeper's
        // stdin remains open until group signal completion, including errors.
        let mut owned = Self {
            child: None,
            keeper: Some(keeper),
            label,
            deadline,
            timeout,
            stdout: None,
            stderr: None,
            ownership: Ownership::Pinned,
            #[cfg(test)]
            lifecycle,
        };
        #[cfg(test)]
        owned.lifecycle.lock().unwrap().extend([
            Lifecycle::KeeperStarted,
            Lifecycle::GuardReady,
            Lifecycle::CommandSpawn,
        ]);
        let group = owned.keeper.as_ref().unwrap().id() as libc::pid_t;
        let child = command
            .process_group(group)
            .stdin(Stdio::null())
            .stdout(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .stderr(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .spawn()?;
        owned.child = Some(child);
        if capture {
            owned.stdout = Some(Capture::drain(
                owned.child.as_mut().unwrap().stdout.take().unwrap(),
            ));
            #[cfg(test)]
            if PANIC_READERS.with(|panic| panic.replace(false)) {
                panic!("owned mock reader startup panic");
            }
            owned.stderr = Some(Capture::drain(
                owned.child.as_mut().unwrap().stderr.take().unwrap(),
            ));
        }
        Ok(owned)
    }

    // Observe liveness without consuming status or releasing the leader's PID.
    // The actual ExitStatus is returned by wait, after group cleanup and reap.
    pub fn try_wait(&mut self) -> io::Result<Option<()>> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{} exceeded {:?}", self.label, self.timeout),
            ));
        }
        self.observe_exit()
    }

    fn observe_exit(&mut self) -> io::Result<Option<()>> {
        if self.ownership != Ownership::Pinned {
            return Err(io::Error::other("owned child identity is no longer pinned"));
        }
        match Self::child_exited(self.keeper.as_ref().unwrap()) {
            Ok(false) => {}
            Ok(true) => {
                return Err(io::Error::other(
                    "owned group lifetime keeper exited unexpectedly",
                ));
            }
            Err(error) => {
                if error.raw_os_error() == Some(libc::ECHILD) {
                    self.ownership = Ownership::Lost;
                    self.keeper.take();
                }
                return Err(error);
            }
        }
        let child = self
            .child
            .as_ref()
            .ok_or_else(|| io::Error::other("actual command identity is unavailable"))?;
        match Self::child_exited(child) {
            Ok(exited) => Ok(exited.then_some(())),
            Err(error) => {
                if error.raw_os_error() == Some(libc::ECHILD) {
                    // The keeper still pins the group, but this actual child
                    // handle is terminal and must never be numerically re-waited.
                    self.child.take();
                }
                Err(error)
            }
        }
    }

    fn child_exited(child: &Child) -> io::Result<bool> {
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        // SAFETY: libc supplies this platform's exact siginfo_t/ABI. The child
        // remains exclusively owned and WNOWAIT explicitly leaves it unreaped.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            return Err(error);
        }
        let info = unsafe { info.assume_init() };
        Ok(unsafe { info.si_pid() } != 0)
    }

    fn signal_and_reap(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut unexpected_keeper_exit = false;
        if self.ownership == Ownership::Lost {
            return Err(io::Error::other(
                "cannot clean up group after owned child identity was lost",
            ));
        }
        if self.ownership == Ownership::Pinned {
            let keeper = self.keeper.as_ref().unwrap();
            match Self::child_exited(keeper) {
                Ok(exited) => unexpected_keeper_exit = exited,
                Err(error) => {
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        self.ownership = Ownership::Lost;
                        self.keeper.take();
                    }
                    return Err(error);
                }
            }
            // A live, exclusively owned keeper makes zombie-only ambiguity
            // impossible on normal paths. No permission error is success.
            #[cfg(test)]
            if let Some(errno) = SIGNAL_ERROR.with(|error| error.take()) {
                return Err(io::Error::from_raw_os_error(errno));
            }
            debug_assert!(
                keeper.stdin.is_some(),
                "keeper lifeline closes only after signal"
            );
            let result = unsafe { libc::killpg(keeper.id() as libc::pid_t, libc::SIGKILL) };
            if result != 0 {
                let error = io::Error::last_os_error();
                return Err(error);
            }
            self.ownership = Ownership::Signaled;
            #[cfg(test)]
            self.lifecycle.lock().unwrap().push(Lifecycle::GroupSignal);
        }
        // Consume each handle before wait. std retries EINTR; any returned error
        // is terminal for that numeric child. Other still-owned children are
        // reaped independently, and Drop cannot retry a lost handle.
        let command = self
            .child
            .take()
            .map(|mut child| {
                #[cfg(test)]
                self.lifecycle.lock().unwrap().push(Lifecycle::CommandWait);
                let result = child.wait();
                #[cfg(test)]
                if result.is_ok() {
                    self.lifecycle.lock().unwrap().push(Lifecycle::CommandReap);
                }
                #[cfg(test)]
                if WAIT_ERROR.with(|error| error.get() == Some(false)) {
                    WAIT_ERROR.with(|error| error.set(None));
                    // Safe lost-identity model: the owned child really was reaped;
                    // no foreign process or PID-reuse experiment is involved.
                    return Err(io::Error::from_raw_os_error(libc::ECHILD));
                }
                result
            })
            .transpose();
        let keeper = self
            .keeper
            .take()
            .map(|mut child| {
                #[cfg(test)]
                self.lifecycle.lock().unwrap().push(Lifecycle::KeeperWait);
                let result = child.wait();
                #[cfg(test)]
                if result.is_ok() {
                    self.lifecycle.lock().unwrap().push(Lifecycle::KeeperReap);
                }
                #[cfg(test)]
                if WAIT_ERROR.with(|error| error.get() == Some(true)) {
                    WAIT_ERROR.with(|error| error.set(None));
                    return Err(io::Error::from_raw_os_error(libc::ECHILD));
                }
                result
            })
            .transpose();
        if command.is_err() || keeper.is_err() {
            self.ownership = Ownership::Lost;
        } else {
            self.ownership = Ownership::Reaped;
        }
        match (command, keeper) {
            (Err(command), Err(keeper)) => Err(io::Error::new(
                command.kind(),
                format!("command wait failed: {command}; keeper wait failed: {keeper}"),
            )),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(_), Ok(_)) if unexpected_keeper_exit => Err(io::Error::other(
                "owned group lifetime keeper exited unexpectedly",
            )),
            (Ok(status), Ok(_)) => Ok(status),
        }
    }

    pub fn wait_for_stdout(&mut self, needle: &str) -> io::Result<()> {
        loop {
            self.stdout
                .as_mut()
                .expect("stdout must be captured")
                .poll()?;
            self.stderr.as_mut().unwrap().poll()?;
            if self.try_wait()?.is_some() {
                return Err(io::Error::other(format!(
                    "{} exited before {needle:?}\nstderr:\n{}",
                    self.label,
                    self.stderr.as_ref().unwrap().tail()
                )));
            }
            let bytes = self.stdout.as_ref().unwrap().bytes.lock().unwrap();
            if String::from_utf8_lossy(&bytes).contains(needle) {
                return Ok(());
            }
            drop(bytes);
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait(mut self) -> io::Result<Output> {
        loop {
            for capture in [&mut self.stdout, &mut self.stderr].into_iter().flatten() {
                if let Err(error) = capture.poll() {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", self.label),
                    ));
                }
            }
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} exceeded {:?}\nstdout tail:\n{}\nstderr tail:\n{}",
                        self.label,
                        self.timeout,
                        self.stdout
                            .as_ref()
                            .map(Capture::tail)
                            .unwrap_or_else(|| "(inherited)".into()),
                        self.stderr
                            .as_ref()
                            .map(Capture::tail)
                            .unwrap_or_else(|| "(inherited)".into())
                    ),
                ));
            }
            if self.observe_exit()?.is_some()
                && self.stdout.as_ref().is_none_or(|capture| capture.finished)
                && self.stderr.as_ref().is_none_or(|capture| capture.finished)
            {
                let status = self
                    .signal_and_reap()?
                    .expect("actual command must be spawned");
                return Ok(Output {
                    status,
                    stdout: self.stdout.as_ref().map(Capture::take).unwrap_or_default(),
                    stderr: self.stderr.as_ref().map(Capture::take).unwrap_or_default(),
                });
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedCommand {
    fn drop(&mut self) {
        // No numeric group is signaled after reap. Remote Docker resources still
        // require their existing named/project-scoped cleanup separately.
        if self.ownership != Ownership::Reaped
            && let Err(error) = self.signal_and_reap()
        {
            eprintln!("{} owned group cleanup/reap failed: {error}", self.label);
        }
    }
}

pub fn run_command(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    OwnedCommand::spawn(command, timeout)?.wait()
}

pub fn run_status(command: &mut Command, timeout: Duration) -> io::Result<ExitStatus> {
    // Preserve live build/setup logs, as Command::status did before bounding it.
    Ok(OwnedCommand::spawn_with_output(command, timeout, false)?
        .wait()?
        .status)
}

pub fn cleanup(command: &mut Command, timeout: Duration) {
    match run_command(command, timeout) {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "test-owned cleanup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => eprintln!("test-owned cleanup failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::Write,
        os::unix::net::UnixStream,
        panic::{self, AssertUnwindSafe},
    };

    const MODE: &str = "IPERF3_RS_COMMAND_TEST_MODE";

    fn fixture_command(mode: &str) -> Command {
        let test = format!("{}::fixture", module_path!().split_once("::").unwrap().1);
        let mut command = Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", &test, "--nocapture", "--test-threads=1"])
            .env(MODE, mode);
        command
    }

    #[test]
    #[allow(clippy::zombie_processes)] // Deliberate orphan: the owned process group must kill it.
    fn fixture() {
        match env::var(MODE).ok().as_deref() {
            None | Some("empty") => {}
            Some("sleep") => {
                eprintln!("stalled fixture marker");
                thread::sleep(Duration::from_secs(60));
            }
            Some("nonzero") => std::process::exit(7),
            Some("large") => {
                io::stdout().write_all(&vec![b'x'; 1024 * 1024]).unwrap();
                io::stderr().write_all(&vec![b'y'; 1024 * 1024]).unwrap();
            }
            Some("too-large") => {
                io::stdout()
                    .write_all(&vec![b'x'; OUTPUT_LIMIT + 1024 * 1024])
                    .unwrap();
            }
            Some("descendant") => {
                // Inherit the owned command's process group and pipes.
                fixture_command("sleep").spawn().unwrap();
            }
            Some("descendant-closed") => {
                fixture_command("sleep")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
            }
            Some(mode) => panic!("unknown command fixture {mode}"),
        }
    }

    #[test]
    fn command_drains_large_stdout_and_stderr() {
        assert!(
            run_status(&mut fixture_command("empty"), Duration::from_secs(5))
                .unwrap()
                .success()
        );
        let output = run_command(&mut fixture_command("large"), Duration::from_secs(5)).unwrap();
        assert!(output.status.success());
        assert!(output.stdout.iter().filter(|&&byte| byte == b'x').count() >= 1024 * 1024);
        assert!(output.stderr.iter().filter(|&&byte| byte == b'y').count() >= 1024 * 1024);
        let error =
            run_command(&mut fixture_command("too-large"), Duration::from_secs(5)).unwrap_err();
        assert!(
            error.to_string().contains("8 MiB"),
            "must fail visibly instead of truncating"
        );
    }

    #[test]
    fn invalid_deadline_is_rejected_before_child_start() {
        let (mut started, mut observer) = UnixStream::pair().unwrap();
        let mut command = fixture_command("empty");
        // Only an async-signal-safe write is performed by this owned exec probe.
        unsafe {
            command.pre_exec(move || started.write_all(b"started"));
        }
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        SPAWN_OBSERVER.with(|observer| *observer.borrow_mut() = Some(Arc::clone(&lifecycle)));
        let error = OwnedCommand::spawn(&mut command, Duration::MAX)
            .err()
            .unwrap();
        SPAWN_OBSERVER.with(|observer| *observer.borrow_mut() = None);
        assert!(
            lifecycle.lock().unwrap().is_empty(),
            "invalid duration starts neither keeper nor command"
        );
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        drop(command);
        let mut bytes = Vec::new();
        observer.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty(), "invalid duration must not start a child");

        let missing = env::current_exe()
            .unwrap()
            .with_extension("missing-exec-fixture");
        let error = OwnedCommand::spawn(&mut Command::new(missing), Duration::from_secs(5))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    fn assert_cleanup_order(lifecycle: &Arc<Mutex<Vec<Lifecycle>>>) {
        assert_eq!(
            *lifecycle.lock().unwrap(),
            [
                Lifecycle::KeeperStarted,
                Lifecycle::GuardReady,
                Lifecycle::CommandSpawn,
                Lifecycle::GroupSignal,
                Lifecycle::CommandWait,
                Lifecycle::CommandReap,
                Lifecycle::KeeperWait,
                Lifecycle::KeeperReap
            ],
            "group cleanup must precede reap with no subsequent signal"
        );
    }

    fn await_exit(child: &mut OwnedCommand) {
        while child.try_wait().unwrap().is_none() {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(child.ownership, Ownership::Pinned);
        // A real second WNOWAIT succeeds only while the owned leader remains
        // unreaped. No kill0/PID-reuse/foreign process experiment is involved.
        assert!(child.observe_exit().unwrap().is_some());
    }

    #[test]
    fn nonzero_and_success_exit_signal_group_before_reap() {
        for (mode, code) in [("nonzero", 7), ("descendant-closed", 0)] {
            let child =
                OwnedCommand::spawn(&mut fixture_command(mode), Duration::from_secs(5)).unwrap();
            let lifecycle = Arc::clone(&child.lifecycle);
            let output = child.wait().unwrap();
            assert_eq!(output.status.code(), Some(code));
            assert_cleanup_order(&lifecycle);
        }
    }

    #[test]
    fn public_poll_pins_identity_until_drop_cleanup() {
        let mut child =
            OwnedCommand::spawn(&mut fixture_command("nonzero"), Duration::from_secs(5)).unwrap();
        let lifecycle = Arc::clone(&child.lifecycle);
        await_exit(&mut child);
        assert!(child.try_wait().unwrap().is_some());
        assert_eq!(
            *lifecycle.lock().unwrap(),
            [
                Lifecycle::KeeperStarted,
                Lifecycle::GuardReady,
                Lifecycle::CommandSpawn
            ],
            "poll must neither signal nor reap"
        );
        drop(child);
        assert_cleanup_order(&lifecycle);
    }

    #[test]
    fn exited_leader_stays_pinned_during_descendant_pipe_and_reader_eof_lag() {
        for mode in ["descendant", "empty"] {
            let mut child =
                OwnedCommand::spawn(&mut fixture_command(mode), Duration::from_millis(500))
                    .unwrap();
            let lifecycle = Arc::clone(&child.lifecycle);
            await_exit(&mut child);
            let (_hold_eof, delayed_done) = mpsc::channel();
            if mode == "empty" {
                // Isolated observer model: the real leader has exited, but the
                // reader completion notification has not yet been delivered.
                child.stdout.as_mut().unwrap().done = delayed_done;
            }
            let error = child.wait().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_cleanup_order(&lifecycle);
        }
    }

    #[test]
    fn keeper_guard_cleans_up_actual_spawn_error_and_reader_startup_panic() {
        let missing = env::current_exe()
            .unwrap()
            .with_extension("missing-owned-command");
        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        SPAWN_OBSERVER.with(|observer| *observer.borrow_mut() = Some(Arc::clone(&lifecycle)));
        let error = OwnedCommand::spawn(&mut Command::new(missing), Duration::from_secs(5))
            .err()
            .unwrap();
        SPAWN_OBSERVER.with(|observer| *observer.borrow_mut() = None);
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            *lifecycle.lock().unwrap(),
            [
                Lifecycle::KeeperStarted,
                Lifecycle::GuardReady,
                Lifecycle::CommandSpawn,
                Lifecycle::GroupSignal,
                Lifecycle::KeeperWait,
                Lifecycle::KeeperReap
            ]
        );

        let lifecycle = Arc::new(Mutex::new(Vec::new()));
        SPAWN_OBSERVER.with(|observer| *observer.borrow_mut() = Some(Arc::clone(&lifecycle)));
        PANIC_READERS.with(|panic| panic.set(true));
        let failure = panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = OwnedCommand::spawn(&mut fixture_command("sleep"), Duration::from_secs(5));
        }));
        SPAWN_OBSERVER.with(|observer| *observer.borrow_mut() = None);
        assert!(failure.is_err());
        assert_cleanup_order(&lifecycle);
    }

    #[test]
    fn unexpected_keeper_exit_and_permission_failure_stay_visible() {
        let mut child =
            OwnedCommand::spawn(&mut fixture_command("sleep"), Duration::from_secs(5)).unwrap();
        let lifecycle = Arc::clone(&child.lifecycle);
        let keeper = child.keeper.as_mut().unwrap();
        // Safe owned mock: read completes naturally, but its stdin handle is
        // deliberately kept open. No unrelated process or OS permission is changed.
        keeper.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !OwnedCommand::child_exited(child.keeper.as_ref().unwrap()).unwrap() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let error = child.try_wait().unwrap_err();
        assert!(error.to_string().contains("keeper exited unexpectedly"));
        assert!(child.keeper.as_ref().unwrap().stdin.is_some());
        drop(child);
        assert_cleanup_order(&lifecycle);

        let mut child =
            OwnedCommand::spawn(&mut fixture_command("sleep"), Duration::from_secs(5)).unwrap();
        let lifecycle = Arc::clone(&child.lifecycle);
        SIGNAL_ERROR.with(|error| error.set(Some(libc::EPERM)));
        let error = child.signal_and_reap().unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        assert_eq!(child.ownership, Ownership::Pinned);
        assert!(child.keeper.as_ref().unwrap().stdin.is_some());
        assert!(child.child.is_some());
        assert_eq!(
            lifecycle.lock().unwrap().len(),
            3,
            "permission failure is not signal success"
        );
        drop(child);
        assert_cleanup_order(&lifecycle);
    }

    #[test]
    fn lost_wait_identity_is_terminal_for_each_child() {
        for keeper in [false, true] {
            let mut child =
                OwnedCommand::spawn(&mut fixture_command("sleep"), Duration::from_secs(5)).unwrap();
            let lifecycle = Arc::clone(&child.lifecycle);
            WAIT_ERROR.with(|error| error.set(Some(keeper)));
            let error = child.signal_and_reap().unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
            assert_eq!(child.ownership, Ownership::Lost);
            assert!(child.child.is_none() && child.keeper.is_none());
            assert_cleanup_order(&lifecycle); // Real owned waits completed before simulated loss.
            let before_drop = lifecycle.lock().unwrap().clone();
            drop(child);
            assert_eq!(
                *lifecycle.lock().unwrap(),
                before_drop,
                "Lost never re-signals or re-waits"
            );
        }
    }

    #[test]
    fn command_timeout_panic_and_cleanup_only_reap_owned_processes() {
        let budget = Duration::from_millis(500);
        let started = Instant::now();
        let mut unrelated =
            OwnedCommand::spawn(&mut fixture_command("sleep"), Duration::from_secs(10)).unwrap();
        let child = OwnedCommand::spawn(&mut fixture_command("sleep"), budget).unwrap();
        let lifecycle = Arc::clone(&child.lifecycle);
        let error = child.wait().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("stalled fixture marker"));
        assert_cleanup_order(&lifecycle);

        let mut panic_lifecycle = None;
        assert!(
            panic::catch_unwind(AssertUnwindSafe(|| {
                let child = OwnedCommand::spawn(&mut fixture_command("sleep"), budget).unwrap();
                panic_lifecycle = Some(Arc::clone(&child.lifecycle));
                panic!("panic before wait");
            }))
            .is_err()
        );
        assert_cleanup_order(&panic_lifecycle.unwrap());
        cleanup(&mut fixture_command("sleep"), budget);
        let error = run_status(&mut fixture_command("sleep"), budget).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let error = run_command(&mut fixture_command("descendant"), budget).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            unrelated.try_wait().unwrap().is_none(),
            "another process group must remain alive"
        );
    }
}

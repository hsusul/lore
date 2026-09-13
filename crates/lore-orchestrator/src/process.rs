//! Process-group supervision for agent processes.
//!
//! Each agent is started as the leader of its own process group, so stopping a
//! task also stops everything it launched (shell commands, test runners, MCP
//! servers). Signals are sent with `/bin/kill` to avoid `unsafe` libc calls.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const GRACE: Duration = Duration::from_secs(3);

fn kill(args: &[&str]) -> bool {
    Command::new("/bin/kill")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether any process in group `pgid` is alive.
pub fn group_alive(pgid: u32) -> bool {
    pgid > 1 && kill(&["-0", "--", &format!("-{pgid}")])
}

/// Whether process `pid` exists.
pub fn group_alive_pid(pid: u32) -> bool {
    pid > 1 && kill(&["-0", &pid.to_string()])
}

/// Send `signal` (e.g. `TERM`) to every process in group `pgid`.
pub fn signal_group(pgid: u32, signal: &str) {
    if pgid > 1 {
        kill(&["-s", signal, "--", &format!("-{pgid}")]);
    }
}

/// Stop group `pgid`: SIGTERM, wait up to a short grace period, then SIGKILL.
/// Reaps `child` (the group leader) when we own it, so it never lingers as a zombie.
pub fn stop_group(pgid: u32, mut child: Option<&mut Child>) {
    signal_group(pgid, "TERM");
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        if let Some(c) = child.as_deref_mut() {
            let _ = c.try_wait();
        }
        if !group_alive(pgid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if group_alive(pgid) {
        signal_group(pgid, "KILL");
    }
    if let Some(c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Whether `pid` is still running the given agent program. Guards against
/// signalling an unrelated process that reused a stale pid after a restart.
pub fn runs_program(pid: u32, program: &Path) -> bool {
    let Some(name) = program.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .map(|out| {
            // The program must be argv[0] or, for interpreters and wrappers
            // (`node .../claude`, `sh fake-agent.sh`), argv[1]; never just a
            // substring of an unrelated command line like `vim claude.md`.
            let line = String::from_utf8_lossy(&out.stdout);
            line.split_whitespace()
                .take(2)
                .any(|word| Path::new(word).file_name().and_then(|n| n.to_str()) == Some(name))
        })
        .unwrap_or(false)
}

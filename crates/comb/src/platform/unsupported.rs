//! Reached only when comb is built for an OS with no implementation. It fails
//! the build on purpose: a stub that quietly succeeds would turn graceful
//! shutdown into "nothing happened" and still look like it worked.

use std::fs::File;
use std::io;
use std::path::Path;
use std::process::ExitStatus;

compile_error!(
    "comb has no process control for this platform. Add crates/comb/src/platform/<os>.rs \
     exporting terminate and describe_exit, and wire it up in platform/mod.rs. \
     Linux is the intended second platform; Windows is out of scope."
);

pub fn terminate(_pid: u32, _signal: crate::spec::StopSignal) -> io::Result<()> {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub fn describe_exit(_status: &ExitStatus) -> String {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub fn try_lock_exclusive(_file: &File) -> io::Result<bool> {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub fn restrict(_path: &Path, _mode: u32) -> io::Result<()> {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub async fn listener_on(_port: u16) -> Option<crate::ports::Listener> {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub async fn build_tools_missing() -> Option<String> {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub fn can_clone(_from: &Path, _into: &Path) -> bool {
    unreachable!("the platform module fails to compile before this is reachable")
}

pub fn clone_directory(_from: &Path, _into: &Path) -> io::Result<()> {
    unreachable!("the platform module fails to compile before this is reachable")
}

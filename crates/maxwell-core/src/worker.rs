//! Running compile and device work. Device I/O always runs in a child process
//! (`<this exe> --worker device`) so a stalled HID call can be killed.

use crate::{Error, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worker {
    Compile,
    Device,
}

/// The app's way to run work; tests substitute a fake.
pub type Runner = Box<dyn FnMut(Worker, &Value) -> Result<Value> + Send>;

pub const TIMEOUT: Duration = Duration::from_secs(15);

const TIMED_OUT: &str = "Worker timed out; device acceptance is unconfirmed. Use Restore saved preset after a Save, or Restore live EQ after a temporary Apply.";
const INVALID: &str = "Worker did not return a valid result. Device acceptance is unconfirmed.";

/// A worker's JSON result, or `Error::Worker` when it reported failure.
fn finish(result: Value, success: bool) -> Result<Value> {
    match result {
        Value::Object(map) if !success || map.contains_key("error") => Err(Error::Worker(map)),
        Value::Object(_) => Ok(result),
        _ => Err(Error::other(INVALID)),
    }
}

/// Compile in this process; errors look like the old worker's `{"error": ...}`.
pub fn compile(request: &Value) -> Result<Value> {
    crate::compile::compile_preset(request).map_err(|error| Error::worker(error.to_string()))
}

/// Run `exe --worker device` with the request on stdin and a hard deadline.
pub fn device_subprocess(exe: &PathBuf, request: &Value, timeout: Duration) -> Result<Value> {
    let mut command = Command::new(exe);
    command.args(["--worker", "device"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child =
        command.spawn().map_err(|error| Error::other(format!("Could not start the device worker: {error}")))?;
    let input = serde_json::to_vec(request)?;
    let mut stdin = child.stdin.take().expect("piped");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::other(TIMED_OUT));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = writer.join();
    let output = reader.join().ok().and_then(|output| output.ok()).unwrap_or_default();
    let result: Value = serde_json::from_slice(&output).map_err(|_| Error::other(INVALID))?;
    finish(result, status.success())
}

/// Compile in-process; device work in a child of `exe`.
pub fn default_runner(exe: PathBuf) -> Runner {
    Box::new(move |worker, request| match worker {
        Worker::Compile => compile(request),
        Worker::Device => device_subprocess(&exe, request, TIMEOUT),
    })
}

/// The `--worker device` entry point: JSON in on stdin, JSON out on stdout.
pub fn device_main() -> i32 {
    let mut input = String::new();
    let request = std::io::stdin()
        .read_to_string(&mut input)
        .map_err(Error::from)
        .and_then(|_| serde_json::from_str::<Value>(input.trim_start_matches('\u{feff}')).map_err(Error::from));
    let (output, success) = match request {
        Ok(request) => crate::device::run(&request),
        Err(error) => (crate::device::failure(&error, &Default::default()), false),
    };
    let mut stdout = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut stdout, &output);
    let _ = stdout.flush();
    if success { 0 } else { 1 }
}

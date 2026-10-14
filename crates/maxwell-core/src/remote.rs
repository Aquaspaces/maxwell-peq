//! Commands for the running app from another process, so
//! `MaxwellPEQ --preset next` works while the app sits in the tray.
//!
//! The app listens on a loopback port and writes the port and a fresh token
//! to `remote.json` in the data folder. A client sends one JSON line carrying
//! the token and gets one JSON line back.

use crate::util::{atomic_json, constant_time_eq, token_urlsafe};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Runs one command; failures come back as `{"error": message}`.
pub type Handler = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

const MAX_REQUEST: u64 = 64 * 1024;
/// Device work has a 15-second deadline of its own, and may queue behind another.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// Start listening and publish how to reach this process.
pub fn listen(data: &Path, handler: Handler) -> std::io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = token_urlsafe();
    let published = json!({"port": port, "token": token, "pid": std::process::id()});
    atomic_json(&crate::paths::remote_path(data), &published)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (handler, token) = (handler.clone(), token.clone());
            std::thread::spawn(move || answer(stream, &token, &handler));
        }
    });
    Ok(())
}

fn answer(mut stream: TcpStream, token: &str, handler: &Handler) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    let read = BufReader::new(&stream).take(MAX_REQUEST).read_line(&mut line);
    let reply = match read.ok().and_then(|_| serde_json::from_str::<Value>(&line).ok()) {
        Some(request) => {
            let given = request.get("token").and_then(Value::as_str).unwrap_or_default();
            if constant_time_eq(given.as_bytes(), token.as_bytes()) {
                handler(&request)
            } else {
                json!({"error": "The command was refused: wrong token."})
            }
        }
        None => json!({"error": "Expected one line of JSON."}),
    };
    let _ = writeln!(stream, "{reply}");
}

/// Send `request` to the running app and return its reply.
pub fn send(data: &Path, request: &Value) -> Result<Value> {
    let unreachable = || Error::value("Maxwell PEQ is running but isn't taking commands. Quit it and try again.");
    let published = std::fs::read_to_string(crate::paths::remote_path(data))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .ok_or_else(unreachable)?;
    let port = published["port"].as_u64().and_then(|port| u16::try_from(port).ok()).ok_or_else(unreachable)?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|_| unreachable())?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT)).map_err(|_| unreachable())?;
    let mut request = request.clone();
    request["token"] = published["token"].clone();
    writeln!(stream, "{request}").map_err(|_| unreachable())?;
    let mut line = String::new();
    BufReader::new(&stream)
        .take(16 * MAX_REQUEST)
        .read_line(&mut line)
        .map_err(|_| Error::value("Maxwell PEQ didn't answer in time."))?;
    let reply: Value =
        serde_json::from_str(&line).map_err(|_| Error::value("Maxwell PEQ sent back an unreadable reply."))?;
    match reply.get("error").and_then(Value::as_str) {
        Some(message) => Err(Error::value(message)),
        None => Ok(reply),
    }
}

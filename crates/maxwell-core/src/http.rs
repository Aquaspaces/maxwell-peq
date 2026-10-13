//! A loopback HTTP server for the editor (`--serve`), for development in a
//! normal browser and for scripted UI checks. The desktop window does not use it.
//!
//! Same rules as the Python server: 127.0.0.1 only, an exact Host header, and
//! a per-run token on every POST. Nothing about requests is logged.

use crate::service::Service;
use crate::util::constant_time_eq;
use serde_json::{Value, json};
use std::io::Read;
use std::sync::Arc;
use tiny_http::{Header, Method, Request, Response, Server};

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'";
const MAX_BODY: usize = 262_144;

macro_rules! web {
    ($name:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/", $name)).as_slice()
    };
}

fn file(path: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match path {
        "/" => (web!("index.html"), "text/html; charset=utf-8"),
        "/style.css" => (web!("style.css"), "text/css; charset=utf-8"),
        "/app.js" => (web!("app.js"), "text/javascript; charset=utf-8"),
        "/graph.js" => (web!("graph.js"), "text/javascript; charset=utf-8"),
        _ => return None,
    })
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn send(request: Request, status: u16, body: Vec<u8>, mime: &str) {
    let response = Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", mime))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Content-Security-Policy", CSP));
    let _ = request.respond(response);
}

fn send_json(request: Request, status: u16, value: &Value) {
    send(request, status, serde_json::to_vec(value).unwrap_or_default(), "application/json; charset=utf-8");
}

fn header_value(request: &Request, name: &'static str) -> Option<String> {
    request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_owned())
}

fn handle(service: &Service, port: u16, mut request: Request) {
    let host = format!("127.0.0.1:{port}");
    if header_value(&request, "Host").as_deref() != Some(&host) {
        return send_json(request, 403, &json!({"error": "Use the 127.0.0.1 address printed by the server."}));
    }
    let path = request.url().to_owned();
    if *request.method() == Method::Get {
        return match path.as_str() {
            "/api/status" => send_json(request, 200, &service.status()),
            "/api/profiles" => send_json(request, 200, &service.profiles()),
            _ => match file(&path) {
                Some((body, mime)) => send(request, 200, body.to_vec(), mime),
                None => send_json(request, 404, &json!({"error": "Not found."})),
            },
        };
    }
    if *request.method() != Method::Post {
        return send_json(request, 405, &json!({"error": "Not found."}));
    }
    let origin = header_value(&request, "Origin");
    let token = header_value(&request, "X-Session-Token").unwrap_or_default();
    if origin.is_some_and(|origin| origin != format!("http://{host}"))
        || !constant_time_eq(token.as_bytes(), service.token.as_bytes())
    {
        return send_json(request, 403, &json!({"error": "Invalid local session. Reload the editor."}));
    }
    let size = request.body_length().unwrap_or(0);
    let json_type = header_value(&request, "Content-Type")
        .is_some_and(|value| value.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"));
    if !(1..=MAX_BODY).contains(&size) || !json_type {
        return send_json(request, 400, &json!({"error": "Expected a JSON request of at most 256 KB."}));
    }
    let mut body = Vec::with_capacity(size);
    if request.as_reader().take(size as u64).read_to_end(&mut body).is_err() {
        return send_json(request, 400, &json!({"error": "Could not read the request."}));
    }
    let parsed = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(error) => return send_json(request, 400, &json!({"error": error.to_string()})),
    };
    match service.post(&path, &parsed) {
        Ok(result) => send_json(request, 200, &result),
        Err(failure) => send_json(request, failure.status, &failure.body()),
    }
}

/// Bind 127.0.0.1:`port` (0 picks a free port); returns the server and its port.
pub fn bind(port: u16) -> std::io::Result<(Server, u16)> {
    let server = Server::http(("127.0.0.1", port)).map_err(std::io::Error::other)?;
    let port = server.server_addr().to_ip().map_or(port, |address| address.port());
    Ok((server, port))
}

/// Serve until the process exits, one thread per request.
pub fn serve(server: Server, port: u16, service: Arc<Service>) {
    for request in server.incoming_requests() {
        let service = service.clone();
        std::thread::spawn(move || handle(&service, port, request));
    }
}

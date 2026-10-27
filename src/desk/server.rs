//! Minimal HTTP server for the desk: `/` serves the page, `/state` the latest
//! JSON state and `/events` streams every update as Server-Sent Events.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

const PAGE: &str = include_str!("index.html");

/// Scripts served next to the page, so the desk works without internet.
const SCRIPTS: &[(&str, &str)] = &[
    ("/orbit3d.js", include_str!("orbit3d.js")),
    ("/agents3d.js", include_str!("agents3d.js")),
    ("/vendor/addons/environments/RoomEnvironment.js", include_str!("vendor/addons/environments/RoomEnvironment.js")),
    ("/vendor/three.module.min.js", include_str!("vendor/three.module.min.js")),
    ("/vendor/addons/controls/OrbitControls.js", include_str!("vendor/addons/controls/OrbitControls.js")),
    ("/vendor/addons/postprocessing/EffectComposer.js", include_str!("vendor/addons/postprocessing/EffectComposer.js")),
    ("/vendor/addons/postprocessing/MaskPass.js", include_str!("vendor/addons/postprocessing/MaskPass.js")),
    ("/vendor/addons/postprocessing/OutputPass.js", include_str!("vendor/addons/postprocessing/OutputPass.js")),
    ("/vendor/addons/postprocessing/Pass.js", include_str!("vendor/addons/postprocessing/Pass.js")),
    ("/vendor/addons/postprocessing/RenderPass.js", include_str!("vendor/addons/postprocessing/RenderPass.js")),
    ("/vendor/addons/postprocessing/ShaderPass.js", include_str!("vendor/addons/postprocessing/ShaderPass.js")),
    ("/vendor/addons/postprocessing/UnrealBloomPass.js", include_str!("vendor/addons/postprocessing/UnrealBloomPass.js")),
    ("/vendor/addons/shaders/CopyShader.js", include_str!("vendor/addons/shaders/CopyShader.js")),
    ("/vendor/addons/shaders/LuminosityHighPassShader.js", include_str!("vendor/addons/shaders/LuminosityHighPassShader.js")),
    ("/vendor/addons/shaders/OutputShader.js", include_str!("vendor/addons/shaders/OutputShader.js")),
];

#[derive(Default)]
struct Inner {
    latest: String,
    clients: Vec<Sender<String>>,
}

/// Fan-out of state updates to all connected browsers.
#[derive(Clone, Default)]
pub struct Hub(Arc<Mutex<Inner>>);

impl Hub {
    pub fn broadcast(&self, json: String) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.clients.retain(|c| c.send(json.clone()).is_ok());
        inner.latest = json;
    }

    fn latest(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .latest
            .clone()
    }

    fn subscribe(&self) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.latest.is_empty() {
            let _ = tx.send(inner.latest.clone());
        }
        inner.clients.push(tx);
        rx
    }
}

/// Bind `addr` and serve in background threads.
pub fn serve(addr: &str, hub: Hub) -> Result<()> {
    let listener = TcpListener::bind(addr).with_context(|| format!("cannot listen on {addr}"))?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let hub = hub.clone();
            thread::spawn(move || {
                let _ = handle(stream, &hub);
            });
        }
    });
    Ok(())
}

fn handle(mut stream: TcpStream, hub: &Hub) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or(path);

    match path {
        "/" | "/index.html" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE),
        "/state" => respond(&mut stream, "200 OK", "application/json", &hub.latest()),
        "/events" => stream_events(stream, hub),
        _ => match SCRIPTS.iter().find(|(p, _)| *p == path) {
            Some((_, body)) => respond(&mut stream, "200 OK", "text/javascript; charset=utf-8", body),
            None => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
        },
    }
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(stream.flush()?)
}

fn stream_events(mut stream: TcpStream, hub: &Hub) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n"
    )?;
    stream.flush()?;
    let rx = hub.subscribe();
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(json) => write!(stream, "data: {json}\n\n")?,
            Err(RecvTimeoutError::Timeout) => write!(stream, ": keep-alive\n\n")?,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        stream.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn serves_state_and_streams_updates() {
        let hub = Hub::default();
        hub.broadcast(r#"{"tick":1}"#.into());
        serve("127.0.0.1:47811", hub.clone()).unwrap();

        let mut s = TcpStream::connect("127.0.0.1:47811").unwrap();
        s.write_all(b"GET /state HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut body = String::new();
        s.read_to_string(&mut body).unwrap();
        assert!(body.starts_with("HTTP/1.1 200 OK") && body.ends_with(r#"{"tick":1}"#));

        let mut s = TcpStream::connect("127.0.0.1:47811").unwrap();
        s.write_all(b"GET /events HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(s);
        let mut seen = Vec::new();
        let mut line = String::new();
        while seen.len() < 2 && reader.read_line(&mut line).unwrap() > 0 {
            if let Some(data) = line.strip_prefix("data: ") {
                seen.push(data.trim().to_string());
                if seen.len() == 1 {
                    hub.broadcast(r#"{"tick":2}"#.into());
                }
            }
            line.clear();
        }
        assert_eq!(seen, vec![r#"{"tick":1}"#, r#"{"tick":2}"#]);
    }
}

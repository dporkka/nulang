//! HTTP/1.1 server built on std::net::TcpListener with httparse.
//!
//! Phase 1: each request dispatches to a standalone VM on the listener thread.
//! Handlers are non-capturing `fn(String) -> String` — no closures with env.
//! Status is always 200; Content-Type is always text/plain.

#[cfg(feature = "tcp")]
use std::io::{Read, Write};
#[cfg(feature = "tcp")]
use std::net::{TcpListener, TcpStream};
#[cfg(feature = "tcp")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "tcp")]
use std::sync::Arc;
#[cfg(feature = "tcp")]
use std::time::Duration;

use crate::bytecode::CodeModule;
#[cfg(feature = "tcp")]
use crate::vm::VM;

/// HTTP method — must match the Nulang-level variant type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "tcp")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

#[cfg(feature = "tcp")]
impl HttpMethod {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "DELETE" => Some(Self::Delete),
            "PATCH" => Some(Self::Patch),
            "HEAD" => Some(Self::Head),
            "OPTIONS" => Some(Self::Options),
            _ => None,
        }
    }
}

#[allow(dead_code)] // Phase 2: method/path/headers will be passed to handlers
#[cfg(feature = "tcp")]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[cfg(feature = "tcp")]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Manages the background HTTP listener thread.
/// Stored on Runtime; `HttpServerState::bind()` spawns the thread.
#[cfg(feature = "tcp")]
pub struct HttpServerState {
    /// Listen port (the actual port, after bind — useful when port 0 is used).
    pub port: u16,
    /// Clone of the handler's module (for per-request VM creation).
    pub handler_module: CodeModule,
    /// Function table index of the handler function within handler_module.
    pub handler_func_idx: usize,
    /// True while the server is running; set to false to signal shutdown.
    shutdown_flag: Arc<AtomicBool>,
    /// Listener thread handle.
    listener_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "tcp")]
impl std::fmt::Debug for HttpServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // JoinHandle is not Debug; print the fields that are.
        f.debug_struct("HttpServerState")
            .field("port", &self.port)
            .field("handler_func_idx", &self.handler_func_idx)
            .field("shutdown_flag", &self.shutdown_flag.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(feature = "tcp")]
impl Drop for HttpServerState {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.listener_thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(feature = "tcp")]
impl HttpServerState {
    const MAX_BODY_SIZE: usize = 1_048_576; // 1 MB

    pub fn bind(
        port: u16,
        handler_module: CodeModule,
        handler_func_idx: usize,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let actual_port = listener.local_addr()?.port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let module_clone = handler_module.clone();

        let handle = std::thread::Builder::new()
            .name("nulang-http-listener".into())
            .spawn(move || {
                Self::listener_loop(listener, module_clone, handler_func_idx, shutdown_clone);
            })?;

        Ok(HttpServerState {
            port: actual_port,
            handler_module,
            handler_func_idx,
            shutdown_flag: shutdown,
            listener_thread: Some(handle),
        })
    }

    fn listener_loop(
        listener: TcpListener,
        handler_module: CodeModule,
        handler_func_idx: usize,
        shutdown: Arc<AtomicBool>,
    ) {
        listener.set_nonblocking(true).ok();
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                    Self::handle_connection(stream, &handler_module, handler_func_idx);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }

    fn handle_connection(
        mut stream: TcpStream,
        handler_module: &CodeModule,
        handler_func_idx: usize,
    ) {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let mut headers = [httparse::EMPTY_HEADER; 64];
                    let mut req = httparse::Request::new(&mut headers);
                    match req.parse(&buf[..n]) {
                        Ok(httparse::Status::Complete(body_offset)) => {
                            let method = HttpMethod::from_str(req.method.unwrap_or("GET"))
                                .unwrap_or(HttpMethod::Get);
                            let path = req.path.unwrap_or("/").to_string();

                            let headers: Vec<(String, String)> = req
                                .headers
                                .iter()
                                .map(|h| {
                                    (
                                        h.name.to_string(),
                                        String::from_utf8_lossy(h.value).to_string(),
                                    )
                                })
                                .collect();

                            let content_length: usize = headers
                                .iter()
                                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, v)| v.parse().ok())
                                .unwrap_or(0);

                            let mut body = Vec::new();
                            if body_offset < n {
                                body.extend_from_slice(&buf[body_offset..n]);
                            }
                            while body.len() < content_length.min(Self::MAX_BODY_SIZE) {
                                let mut chunk = [0u8; 4096];
                                match stream.read(&mut chunk) {
                                    Ok(0) => break,
                                    Ok(m) => body.extend_from_slice(&chunk[..m]),
                                    Err(_) => break,
                                }
                            }

                            if content_length > Self::MAX_BODY_SIZE {
                                Self::write_response(
                                    &mut stream,
                                    &HttpResponse {
                                        status: 413,
                                        headers: vec![("Content-Type".into(), "text/plain".into())],
                                        body: b"Payload too large".to_vec(),
                                    },
                                );
                                break;
                            }

                            let request = HttpRequest {
                                method,
                                path,
                                headers,
                                body,
                            };
                            let response =
                                Self::dispatch(handler_module, handler_func_idx, &request);
                            let keep_alive = false; // Phase 1: close after each request
                            Self::write_response(&mut stream, &response);
                            if !keep_alive {
                                break;
                            }
                        }
                        Ok(httparse::Status::Partial) => continue, // need more data
                        Err(_) => {
                            Self::write_response(
                                &mut stream,
                                &HttpResponse {
                                    status: 400,
                                    headers: vec![("Content-Type".into(), "text/plain".into())],
                                    body: b"Bad request".to_vec(),
                                },
                            );
                            break;
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    fn write_response(stream: &mut TcpStream, response: &HttpResponse) {
        let status_text = match response.status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            _ => "Unknown",
        };
        let mut out = format!("HTTP/1.1 {} {}\r\n", response.status, status_text);
        for (k, v) in &response.headers {
            out.push_str(&format!("{}: {}\r\n", k, v));
        }
        out.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
        out.push_str("\r\n");
        let _ = stream.write_all(out.as_bytes());
        let _ = stream.write_all(&response.body);
        let _ = stream.flush();
    }

    /// Dispatch a request to the handler via a fresh standalone VM.
    ///
    /// Clones the handler module, injects the request body as a string constant,
    /// emits trampoline bytecode that loads the body into r0, creates a closure
    /// for the handler function, calls it, and returns. The handler receives the
    /// body string in r0 and is expected to return a string in r0.
    fn dispatch(
        handler_module: &CodeModule,
        handler_func_idx: usize,
        request: &HttpRequest,
    ) -> HttpResponse {
        use crate::bytecode::{Constant, Instruction, OpCode};

        let mut vm = VM::new();
        let mut module = handler_module.clone();

        let body_str = String::from_utf8_lossy(&request.body).to_string();
        let body_idx = module.add_constant(Constant::String(body_str));

        let entry_offset = module.instructions.len();
        // ConstU body_idx -> r0 (first argument to handler)
        module.emit(Instruction::new3(
            OpCode::ConstU,
            ((body_idx >> 8) & 0xFF) as u8,
            (body_idx & 0xFF) as u8,
            0,
        ));
        // Closure handler_func_idx -> r1 (function reference)
        module.emit(Instruction::new3(
            OpCode::Closure,
            ((handler_func_idx >> 8) & 0xFF) as u8,
            (handler_func_idx & 0xFF) as u8,
            1,
        ));
        // ClosureCall r1, 0, r0 — call closure in r1, result -> r0
        module.emit(Instruction::new3(OpCode::ClosureCall, 1, 0, 0));
        // Ret — pops frame, returns r0 to VM
        module.emit(Instruction::new0(OpCode::Ret));

        vm.load_module(module);
        match vm.run_from(0, entry_offset) {
            Ok(result) => {
                let body = vm.value_to_string(0, result);
                HttpResponse {
                    status: 200,
                    headers: vec![("Content-Type".into(), "text/plain".into())],
                    body: body.into_bytes(),
                }
            }
            Err(_) => HttpResponse {
                status: 500,
                headers: vec![("Content-Type".into(), "text/plain".into())],
                body: b"Internal server error".to_vec(),
            },
        }
    }
}


#[cfg(not(feature = "tcp"))]
pub struct HttpServerState {
    /// Listen port (stub: always 0; `bind` fails without the `tcp` feature).
    pub port: u16,
}

#[cfg(not(feature = "tcp"))]
impl HttpServerState {
    pub fn bind(
        _port: u16,
        _handler_module: CodeModule,
        _handler_func_idx: usize,
    ) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "HTTP server disabled (feature 'tcp' not enabled)",
        ))
    }
}
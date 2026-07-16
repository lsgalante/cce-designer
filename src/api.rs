use std::net::TcpListener;
use std::io::{BufRead, BufReader, Read, Write};
use crate::{CustomEvent, HttpAction};

pub fn start_http_server(server_sender: calloop::channel::Sender<CustomEvent>) {
    std::thread::spawn(move || {
        // CCE_DESIGNER_HTTP_PORT overrides the default so a second instance
        // (tests, debugging) can run alongside one already holding 3000.
        let port: u16 = std::env::var("CCE_DESIGNER_HTTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Failed to bind HTTP server to port {port}: {:?}", e);
                return;
            }
        };
        println!("Embedded HTTP Server listening on http://127.0.0.1:{port}");

        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };

            let server_sender = server_sender.clone();
            std::thread::spawn(move || {
                let mut write_stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }

                if request_line.starts_with("GET /state") {
                    let (tx, rx) = std::sync::mpsc::channel();
                    if server_sender.send(CustomEvent::GetState(tx)).is_ok() {
                        let response_body = rx.recv().unwrap_or_else(|_| "null".to_string());
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response_body.len(),
                            response_body
                        );
                        let _ = write_stream.write_all(response.as_bytes());
                    }
                } else if request_line.starts_with("POST /action") {
                    let mut content_length = 0;
                    loop {
                        let mut header_line = String::new();
                        if reader.read_line(&mut header_line).is_err() || header_line == "\r\n" || header_line == "\n" || header_line.is_empty() {
                            break;
                        }
                        let lower = header_line.to_lowercase();
                        if lower.starts_with("content-length:") {
                            if let Some(val) = lower.split(':').nth(1) {
                                if let Ok(len) = val.trim().parse::<usize>() {
                                    content_length = len;
                                }
                            }
                        }
                    }

                    let mut body = vec![0; content_length];
                    if reader.read_exact(&mut body).is_ok() {
                        let body_str = String::from_utf8_lossy(&body);
                        if let Ok(action) = serde_json::from_str::<HttpAction>(&body_str) {
                            let (tx, rx) = std::sync::mpsc::channel();
                            if server_sender.send(CustomEvent::PostAction(action, tx)).is_ok() {
                                let res = rx.recv().unwrap_or_else(|_| Err("internal error".to_string()));
                                let response = match res {
                                    Ok(msg) => {
                                        let body = format!("{{\"status\":\"success\",\"message\":\"{}\"}}", msg);
                                        format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                            body.len(),
                                            body
                                        )
                                    }
                                    Err(err) => {
                                        let body = format!("{{\"status\":\"error\",\"error\":\"{}\"}}", err);
                                        format!(
                                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                            body.len(),
                                            body
                                        )
                                    }
                                };
                                let _ = write_stream.write_all(response.as_bytes());
                            } else {
                                let body = "{\"status\":\"error\",\"error\":\"failed to send action to event loop\"}";
                                let response = format!(
                                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = write_stream.write_all(response.as_bytes());
                            }
                        } else {
                            let body = "{\"status\":\"error\",\"error\":\"failed to parse action JSON\"}";
                            let response = format!(
                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = write_stream.write_all(response.as_bytes());
                        }
                    } else {
                        let body = "{\"status\":\"error\",\"error\":\"failed to read complete body\"}";
                        let response = format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = write_stream.write_all(response.as_bytes());
                    }
                } else {
                    let body = "{\"error\":\"not found\"}";
                    let response = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = write_stream.write_all(response.as_bytes());
                }
                let _ = write_stream.flush();
            });
        }
    });
}

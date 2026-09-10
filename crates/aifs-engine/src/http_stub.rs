//! One-shot localhost HTTP stubs for tests.
//!
//! Reads headers and `Content-Length` before writing a reply so Windows/macOS
//! clients are not reset mid-POST (WSAECONNABORTED / EINVAL).

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::Duration;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Serves one JSON response. The join handle yields the raw HTTP request.
pub(crate) fn serve_json_once(
    status: &'static str,
    body: &'static str,
) -> (String, std::thread::JoinHandle<String>) {
    serve_once(status, "application/json", body.as_bytes().to_vec(), "/v1")
}

/// Serves one JSON response from an owned body (dynamic asset ids in tests).
pub(crate) fn serve_json_once_owned(
    status: &'static str,
    body: String,
) -> (String, std::thread::JoinHandle<String>) {
    serve_once(status, "application/json", body.into_bytes(), "/v1")
}

/// Serves one 302 after reading the inbound request. URL is the GET target.
pub(crate) fn serve_redirect_once(location: String) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap_or_else(|error| panic!("{error}"));
        let request = read_http_request(&mut stream).unwrap_or_default();
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        graceful_close(&mut stream, response.as_bytes());
        request
    });
    (format!("http://{addr}/v1/models"), handle)
}

fn serve_once(
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
    path: &'static str,
) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("{error}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap_or_else(|error| panic!("{error}"));
        let request = read_http_request(&mut stream).unwrap_or_default();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut bytes = response.into_bytes();
        bytes.extend_from_slice(&body);
        graceful_close(&mut stream, &bytes);
        request
    });
    (format!("http://{addr}{path}"), handle)
}

pub(crate) fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 2048];
    loop {
        let read = stream.read(&mut tmp)?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..read]);
        if buf.len() > MAX_REQUEST_BYTES {
            break;
        }
        let Some(header_end) = find_subslice(&buf, b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&buf[..header_end]).unwrap_or("");
        let needed = header_end + 4 + content_length(headers);
        while buf.len() < needed && buf.len() <= MAX_REQUEST_BYTES {
            let read = stream.read(&mut tmp)?;
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read]);
        }
        break;
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn graceful_close(stream: &mut TcpStream, response: &[u8]) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = stream.write_all(response);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    let mut sink = [0_u8; 256];
    while stream.read(&mut sink).unwrap_or(0) > 0 {}
}

fn content_length(headers: &str) -> usize {
    for line in headers.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

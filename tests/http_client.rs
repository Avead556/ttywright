#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use ttywright::core::http::{HttpOutcome, HttpResponse, HttpTemplate, request_at};
use ttywright::services::http::{HttpHost, HttpLimits};

fn serve(answer: impl FnOnce(&mut TcpStream) + Send + 'static) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = tx.send(read_request(&mut stream));
        answer(&mut stream);
    });
    (url, rx)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut seen = Vec::new();
    let mut byte = [0; 1024];
    loop {
        let n = stream.read(&mut byte).unwrap();
        seen.extend_from_slice(&byte[..n]);
        let text = String::from_utf8_lossy(&seen).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let wanted = head
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length: ")?
                        .parse()
                        .ok()
                })
                .unwrap_or(0_usize);
            if body.len() >= wanted {
                return text;
            }
        }
        if n == 0 {
            return text;
        }
    }
}

fn template(text: &str) -> HttpTemplate {
    request_at(text, 0).unwrap().unwrap()
}

fn start(host: &HttpHost, root: &Path, text: &str) -> Receiver<HttpOutcome> {
    let (tx, rx): (Sender<HttpOutcome>, _) = channel();
    host.send(
        1,
        root.join("api.http"),
        root.to_path_buf(),
        template(text),
        move |_, outcome| tx.send(outcome).unwrap(),
    );
    rx
}

fn done(outcome: HttpOutcome) -> HttpResponse {
    match outcome {
        HttpOutcome::Done(response) => response,
        other => panic!("not an answer: {other:?}"),
    }
}

const WAIT: Duration = Duration::from_secs(30);

#[test]
fn a_post_arrives_as_written_with_its_variables_and_the_answer_comes_back_clean() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("http-client.private.env"), "TOKEN=s3cret\n").unwrap();
    let (url, asked) = serve(|s| {
        s.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 18\r\nX-Evil: \x1b[31mred\r\n\r\n\x1b]0;pwn\x07hello\nworld\n\n\n")
            .unwrap();
    });
    let host = HttpHost::default();
    let text = format!(
        "POST {url}/a?x={{{{y}}}}&q=[1]\nAuthorization: Bearer {{{{TOKEN}}}}\nContent-Type: text/plain\n\n@not-a-file\n\"line\" two\n"
    )
    .replace("{{y}}", "7");
    let answer = done(start(&host, &root, &text).recv_timeout(WAIT).unwrap());
    let request = asked.recv_timeout(WAIT).unwrap();
    assert!(
        request.starts_with("POST /a?x=7&q=[1] HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        request.contains("Authorization: Bearer s3cret\r\n"),
        "{request}"
    );
    assert!(
        request.ends_with("\r\n\r\n@not-a-file\n\"line\" two"),
        "{request}"
    );
    assert_eq!(answer.code, 201);
    assert!(
        answer.body.iter().any(|l| l.contains("hello")),
        "{:?}",
        answer.body
    );
    let all = format!("{:?}{:?}", answer.headers, answer.body);
    assert!(!all.contains('\u{1b}') && !all.contains('\u{7}'), "{all}");
}

#[test]
fn a_body_without_a_type_is_not_called_a_form_and_head_does_not_hang() {
    let dir = tempfile::tempdir().unwrap();
    let (url, asked) = serve(|s| s.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap());
    let host = HttpHost::default();
    let text = format!("PUT {url}/\n\nbody\n");
    assert_eq!(
        done(start(&host, dir.path(), &text).recv_timeout(WAIT).unwrap()).code,
        204
    );
    assert!(
        !asked
            .recv_timeout(WAIT)
            .unwrap()
            .to_lowercase()
            .contains("content-type")
    );

    let (url, _asked) = serve(|s| {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n")
            .unwrap();
        thread::sleep(Duration::from_secs(20));
    });
    let text = format!("HEAD {url}/\n");
    let started = Instant::now();
    assert_eq!(
        done(start(&host, dir.path(), &text).recv_timeout(WAIT).unwrap()).code,
        200
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_redirect_is_shown_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let (url, _asked) = serve(|s| {
        s.write_all(
            b"HTTP/1.1 302 Found\r\nLocation: file:///etc/passwd\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();
    });
    let host = HttpHost::default();
    let answer = done(
        start(&host, dir.path(), &format!("GET {url}/\n"))
            .recv_timeout(WAIT)
            .unwrap(),
    );
    assert_eq!(answer.code, 302);
}

fn hanging_server() -> String {
    let (url, _asked) = serve(|_| thread::sleep(Duration::from_secs(60)));
    url
}

#[test]
fn stop_ends_a_request_that_gets_no_answer() {
    let dir = tempfile::tempdir().unwrap();
    let host = HttpHost::default();
    let rx = start(&host, dir.path(), &format!("GET {}/\n", hanging_server()));
    thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    host.cancel(1);
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), HttpOutcome::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn stop_right_after_send_is_not_lost() {
    let dir = tempfile::tempdir().unwrap();
    let host = HttpHost::default();
    let rx = start(&host, dir.path(), &format!("GET {}/\n", hanging_server()));
    host.cancel(1);
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), HttpOutcome::Cancelled);
}

#[test]
fn the_time_limit_ends_it_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let host = HttpHost::with_limits(HttpLimits {
        max_seconds: 1,
        ..HttpLimits::default()
    });
    let rx = start(&host, dir.path(), &format!("GET {}/\n", hanging_server()));
    let HttpOutcome::Failed(message) = rx.recv_timeout(WAIT).unwrap() else {
        panic!("should have failed");
    };
    assert!(message.to_lowercase().contains("timed out"), "{message}");
}

#[test]
fn an_endless_answer_stops_at_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let (url, _asked) = serve(|s| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
        let chunk = [b'x'; 4096];
        while s.write_all(&chunk).is_ok() {}
    });
    let host = HttpHost::with_limits(HttpLimits {
        max_bytes: 100_000,
        ..HttpLimits::default()
    });
    let answer = done(
        start(&host, dir.path(), &format!("GET {url}/\n"))
            .recv_timeout(WAIT)
            .unwrap(),
    );
    assert!(answer.truncated);
    assert!(answer.body_bytes < 100_000);
}

#[test]
fn nothing_listening_is_reported_and_a_missing_variable_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let free = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let host = HttpHost::default();
    let rx = start(&host, dir.path(), &format!("GET http://{free}/\n"));
    assert!(matches!(
        rx.recv_timeout(WAIT).unwrap(),
        HttpOutcome::Failed(_)
    ));
    let rx = start(&host, dir.path(), "GET http://{{nope}}/\n");
    let HttpOutcome::Failed(message) = rx.recv_timeout(WAIT).unwrap() else {
        panic!("should have failed");
    };
    assert!(message.contains("nope"), "{message}");
}

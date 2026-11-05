use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::run_output::OutputDecoder;

const METHODS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE", "CONNECT",
];
const MAX_EXPANSION_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpTemplate {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub vars: Vec<(String, String)>,
}

impl HttpTemplate {
    pub fn title(&self) -> String {
        format!("{} {}", self.method, self.url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

fn is_request_end(line: &str) -> bool {
    line.starts_with("###")
}

fn is_comment(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('#') || line.starts_with("//")
}

fn file_var(line: &str) -> Option<(String, String)> {
    let (name, value) = line.strip_prefix('@')?.split_once('=')?;
    Some((name.trim().to_owned(), value.trim().to_owned()))
}

fn request_line(line: &str) -> (String, String) {
    let line = line.trim();
    let (method, rest) = match line.split_once(char::is_whitespace) {
        Some((first, rest)) if METHODS.contains(&first) => (first, rest.trim()),
        _ if METHODS.contains(&line) => (line, ""),
        _ => ("GET", line),
    };
    let url = match rest.rsplit_once(char::is_whitespace) {
        Some((url, version)) if version.starts_with("HTTP/") => url.trim_end(),
        _ => rest,
    };
    (method.to_owned(), url.to_owned())
}

fn ends_body(line: &str) -> bool {
    line.starts_with("> {%") || line.starts_with(">>") || line.starts_with("<> ")
}

fn parse_block(
    lines: &[&str],
    vars: &mut Vec<(String, String)>,
) -> Result<Option<HttpTemplate>, String> {
    let mut rest = lines.iter().copied().peekable();
    let (method, mut url) = loop {
        let Some(line) = rest.next() else {
            return Ok(None);
        };
        if let Some(var) = file_var(line) {
            vars.push(var);
        } else if !line.trim().is_empty() && !is_comment(line) {
            break request_line(line);
        }
    };
    while let Some(line) = rest.next_if(|l| l.starts_with([' ', '\t']) && !l.trim().is_empty()) {
        url.push_str(line.trim());
    }
    let mut headers = Vec::new();
    for line in rest.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        if is_comment(line) {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("This is not a header: {}", line.trim()));
        };
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    let mut body: Vec<&str> = rest.take_while(|l| !ends_body(l)).collect();
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    if body.first().is_some_and(|l| l.starts_with("< ")) {
        return Err("A body read from a file is not supported".to_owned());
    }
    Ok(Some(HttpTemplate {
        method,
        url,
        headers,
        body: (!body.is_empty()).then(|| body.join("\n")),
        vars: vars.clone(),
    }))
}

pub fn request_at(text: &str, line: usize) -> Result<Option<HttpTemplate>, String> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut vars = Vec::new();
    let mut start = 0;
    loop {
        let end = lines[start..]
            .iter()
            .position(|l| is_request_end(l))
            .map_or(lines.len(), |n| start + n);
        if line <= end || end == lines.len() {
            return parse_block(&lines[start..end], &mut vars);
        }
        let _ = parse_block(&lines[start..end], &mut vars);
        start = end + 1;
    }
}

pub fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

fn expand(text: &str, vars: &[(String, String)], depth: usize) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("{{") {
        out.push_str(before);
        let Some((name, tail)) = after.split_once("}}") else {
            out.push_str("{{");
            rest = after;
            continue;
        };
        let name = name.trim();
        let Some((_, value)) = vars.iter().rev().find(|(n, _)| n == name) else {
            return Err(format!("No value for {{{{{name}}}}}"));
        };
        if depth == 0 {
            return Err(format!("{{{{{name}}}}} refers to itself"));
        }
        out.push_str(&expand(value, vars, depth - 1)?);
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

pub fn resolve(template: &HttpTemplate, env: &[(String, String)]) -> Result<HttpRequest, String> {
    let mut vars = template.vars.clone();
    vars.extend_from_slice(env);
    let fill = |text: &str| expand(text, &vars, MAX_EXPANSION_DEPTH);
    let url = fill(&template.url)?;
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("The URL must start with http:// or https://".to_owned());
    }
    if url.chars().any(|c| c.is_control() || c == ' ') {
        return Err("The URL has a space or a control character in it".to_owned());
    }
    let mut headers = Vec::with_capacity(template.headers.len());
    for (name, value) in &template.headers {
        let (name, value) = (fill(name)?, fill(value)?);
        if !is_token(&name) {
            return Err("A header name is not valid".to_owned());
        }
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(format!("Header {name} has a control character in it"));
        }
        headers.push((name, value));
    }
    let body = template.body.as_deref().map(fill).transpose()?;
    if body.as_deref().is_some_and(|b| b.contains('\0')) {
        return Err("The body has a NUL character in it".to_owned());
    }
    Ok(HttpRequest {
        method: template.method.clone(),
        url,
        headers,
        body,
    })
}

fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn curl_config(request: &HttpRequest) -> String {
    let mut out = format!("url = {}\n", quoted(&request.url));
    if request.method == "HEAD" {
        out.push_str("head\n");
    } else {
        out.push_str(&format!("request = {}\n", quoted(&request.method)));
    }
    for (name, value) in &request.headers {
        let line = if value.is_empty() {
            format!("{name};")
        } else {
            format!("{name}: {value}")
        };
        out.push_str(&format!("header = {}\n", quoted(&line)));
    }
    if let Some(body) = &request.body {
        let typed = request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        if !typed {
            out.push_str("header = \"Content-Type:\"\n");
        }
        out.push_str(&format!("data-raw = {}\n", quoted(body)));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: String,
    pub code: u16,
    pub headers: Vec<String>,
    pub body: Vec<String>,
    pub body_bytes: usize,
    pub binary: bool,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpOutcome {
    Done(HttpResponse),
    Failed(String),
    Cancelled,
}

pub fn clean_lines(bytes: &[u8]) -> Vec<String> {
    let mut decoder = OutputDecoder::default();
    let mut lines = Vec::new();
    decoder.feed_styled(bytes, |text, _| lines.push(text));
    let partial = decoder.partial();
    if !partial.is_empty() {
        lines.push(partial);
    }
    lines
}

fn split_head(bytes: &[u8]) -> (&[u8], &[u8]) {
    for i in 0..bytes.len() {
        if bytes[i..].starts_with(b"\r\n\r\n") {
            return (&bytes[..i], &bytes[i + 4..]);
        }
        if bytes[i..].starts_with(b"\n\n") {
            return (&bytes[..i], &bytes[i + 2..]);
        }
    }
    (bytes, &[])
}

pub fn parse_response(
    bytes: &[u8],
    truncated: bool,
    elapsed_ms: u64,
) -> Result<HttpResponse, String> {
    let mut rest = bytes;
    loop {
        if !rest.starts_with(b"HTTP/") {
            return Err("The answer is not HTTP".to_owned());
        }
        let (head, body) = split_head(rest);
        let mut lines = clean_lines(head).into_iter();
        let status = lines.next().unwrap_or_default();
        let code = status
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or("The answer is not HTTP")?;
        let tunnel = status
            .to_ascii_lowercase()
            .contains("connection established");
        if ((100..200).contains(&code) || tunnel) && body.starts_with(b"HTTP/") {
            rest = body;
            continue;
        }
        let binary = body.iter().take(8192).any(|b| *b == 0);
        return Ok(HttpResponse {
            status,
            code,
            headers: lines.collect(),
            body: if binary {
                Vec::new()
            } else {
                clean_lines(body)
            },
            body_bytes: body.len(),
            binary,
            truncated,
            elapsed_ms,
        });
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpPanel {
    pub current: Option<u64>,
    pub title: String,
    pub outcome: Option<HttpOutcome>,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpRowKind {
    Status,
    Failure,
    Header,
    Body,
    Note,
}

impl HttpPanel {
    pub fn rows(&self) -> Vec<(HttpRowKind, String)> {
        let response = match &self.outcome {
            None => return Vec::new(),
            Some(HttpOutcome::Cancelled) => {
                return vec![(HttpRowKind::Note, "Stopped".to_owned())];
            }
            Some(HttpOutcome::Failed(message)) => {
                return vec![(HttpRowKind::Failure, message.clone())];
            }
            Some(HttpOutcome::Done(response)) => response,
        };
        let mut rows = vec![(
            HttpRowKind::Status,
            format!(
                "{}   {} ms   {} bytes",
                response.status, response.elapsed_ms, response.body_bytes
            ),
        )];
        rows.extend(
            response
                .headers
                .iter()
                .map(|h| (HttpRowKind::Header, h.clone())),
        );
        rows.push((HttpRowKind::Body, String::new()));
        if response.binary {
            rows.push((HttpRowKind::Note, "Binary body, not shown".to_owned()));
        }
        rows.extend(response.body.iter().map(|l| (HttpRowKind::Body, l.clone())));
        if response.truncated {
            rows.push((
                HttpRowKind::Note,
                "Too long: the rest was not read".to_owned(),
            ));
        }
        rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpEffect {
    Send {
        id: u64,
        file: PathBuf,
        root: PathBuf,
        template: HttpTemplate,
    },
    Cancel {
        id: u64,
    },
}

impl HttpEffect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Send { .. } => true,
            Self::Cancel { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpEvent {
    Finished { id: u64, outcome: HttpOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpState {
    panel: HttpPanel,
    next_id: u64,
}

impl Default for HttpState {
    fn default() -> Self {
        Self {
            panel: HttpPanel::default(),
            next_id: 1,
        }
    }
}

impl HttpState {
    pub const fn panel(&self) -> &HttpPanel {
        &self.panel
    }

    pub fn send(
        &mut self,
        file: PathBuf,
        root: PathBuf,
        template: HttpTemplate,
    ) -> Vec<HttpEffect> {
        let mut fx = Vec::new();
        if let Some(id) = self.panel.current.take() {
            fx.push(HttpEffect::Cancel { id });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.panel = HttpPanel {
            current: Some(id),
            title: template.title(),
            outcome: None,
            scroll: 0,
        };
        fx.push(HttpEffect::Send {
            id,
            file,
            root,
            template,
        });
        fx
    }

    pub const fn stop(&self) -> Option<HttpEffect> {
        match self.panel.current {
            Some(id) => Some(HttpEffect::Cancel { id }),
            None => None,
        }
    }

    pub fn handle(&mut self, event: HttpEvent) {
        let HttpEvent::Finished { id, outcome } = event;
        if self.panel.current != Some(id) {
            return;
        }
        self.panel.current = None;
        self.panel.outcome = Some(outcome);
        self.panel.scroll = 0;
    }

    pub fn scroll(&mut self, delta: isize) {
        let last = self.panel.rows().len().saturating_sub(1);
        self.panel.scroll = self.panel.scroll.saturating_add_signed(delta).min(last);
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn template(line: &str) -> HttpTemplate {
        request_at(line, 0).unwrap().unwrap()
    }

    fn sent_id(fx: &[HttpEffect]) -> u64 {
        fx.iter()
            .find_map(|e| match e {
                HttpEffect::Send { id, .. } => Some(*id),
                HttpEffect::Cancel { .. } => None,
            })
            .unwrap()
    }

    fn done(code: u16) -> HttpOutcome {
        HttpOutcome::Done(HttpResponse {
            status: format!("HTTP/1.1 {code}"),
            code,
            headers: vec!["A: b".to_owned()],
            body: vec!["one".to_owned(), "two".to_owned()],
            body_bytes: 8,
            elapsed_ms: 1,
            binary: false,
            truncated: false,
        })
    }

    #[test]
    fn a_second_send_cancels_the_first_and_ignores_its_answer() {
        let mut http = HttpState::default();
        let first = sent_id(&http.send("a.http".into(), ".".into(), template("GET http://a")));
        let fx = http.send("a.http".into(), ".".into(), template("GET http://b"));
        assert_eq!(fx[0], HttpEffect::Cancel { id: first });
        let second = sent_id(&fx);
        assert_ne!(first, second);

        http.handle(HttpEvent::Finished {
            id: first,
            outcome: done(500),
        });
        assert_eq!(http.panel().current, Some(second));
        assert!(http.panel().outcome.is_none());

        http.handle(HttpEvent::Finished {
            id: second,
            outcome: done(200),
        });
        assert_eq!(http.panel().current, None);
        assert_eq!(http.panel().outcome, Some(done(200)));
    }

    #[test]
    fn stop_names_the_running_request_only() {
        let mut http = HttpState::default();
        assert_eq!(http.stop(), None);
        let id = sent_id(&http.send("a.http".into(), ".".into(), template("GET http://a")));
        assert_eq!(http.stop(), Some(HttpEffect::Cancel { id }));
    }

    #[test]
    fn scrolling_stays_within_the_rows() {
        let mut http = HttpState::default();
        let id = sent_id(&http.send("a.http".into(), ".".into(), template("GET http://a")));
        http.handle(HttpEvent::Finished {
            id,
            outcome: done(200),
        });
        http.scroll(-5);
        assert_eq!(http.panel().scroll, 0);
        http.scroll(100);
        assert_eq!(http.panel().scroll, http.panel().rows().len() - 1);
    }

    #[test]
    fn only_sending_needs_trust() {
        let send = HttpEffect::Send {
            id: 1,
            file: "a.http".into(),
            root: ".".into(),
            template: template("GET http://a"),
        };
        assert!(send.needs_trust());
        assert!(!HttpEffect::Cancel { id: 1 }.needs_trust());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "@host = http://a\n\n# first\nGET {{host}}/one HTTP/1.1\nAccept: text/plain\n\n###\nPOST {{host}}/two\n  ?x=1\n  &y=2\n// note\nContent-Type: application/json\n\n{\"a\": 1}\n\n> {% client.log(1) %}\n\n### third\n\nhttp://b/three\n";

    fn at(line: usize) -> HttpTemplate {
        request_at(FILE, line).unwrap().unwrap()
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn the_cursor_chooses_the_block() {
        let first = at(3);
        assert_eq!(
            (first.method.as_str(), first.url.as_str()),
            ("GET", "{{host}}/one")
        );
        assert_eq!(first.headers, pairs(&[("Accept", "text/plain")]));
        assert_eq!(first.body, None);
        assert_eq!(at(0), first, "the comment above it belongs to it");
        let second = at(13);
        assert_eq!(second.method, "POST");
        assert_eq!(second.url, "{{host}}/two?x=1&y=2");
        assert_eq!(second.headers.len(), 1, "a comment is no header");
        assert_eq!(
            second.body.as_deref(),
            Some("{\"a\": 1}"),
            "the script is not sent"
        );
        assert_eq!(
            second.vars,
            pairs(&[("host", "http://a")]),
            "seen from above"
        );
        let third = at(19);
        assert_eq!(
            (third.method.as_str(), third.url.as_str()),
            ("GET", "http://b/three")
        );
    }

    #[test]
    fn a_block_with_no_request_is_none_and_a_broken_one_says_why() {
        assert_eq!(request_at("# only a comment\n\n", 0), Ok(None));
        assert_eq!(request_at("", 0), Ok(None));
        let err = request_at("GET http://a\nnot a header\n", 0).unwrap_err();
        assert!(err.contains("not a header"), "{err}");
        assert!(request_at("POST http://a\n\n< ./secret.txt\n", 0).is_err());
    }

    #[test]
    fn a_broken_block_elsewhere_does_not_stop_this_one() {
        let text = "GET http://a\nbroken\n###\nGET http://b\n";
        assert_eq!(request_at(text, 3).unwrap().unwrap().url, "http://b");
    }

    #[test]
    fn crlf_files_read_the_same() {
        let got = request_at("POST http://a\r\nA: b\r\n\r\nbody\r\n", 0)
            .unwrap()
            .unwrap();
        assert_eq!(got.url, "http://a");
        assert_eq!(got.headers, pairs(&[("A", "b")]));
        assert_eq!(got.body.as_deref(), Some("body"));
    }

    #[test]
    fn the_environment_wins_and_variables_may_use_variables() {
        let mut t = at(3);
        t.vars = pairs(&[("host", "http://file"), ("base", "{{host}}/api")]);
        t.url = "{{ base }}/x".to_owned();
        let env = pairs(&[("host", "http://env")]);
        assert_eq!(resolve(&t, &env).unwrap().url, "http://env/api/x");
    }

    #[test]
    fn a_missing_variable_names_itself_and_a_loop_ends() {
        let mut t = at(3);
        t.url = "http://a/{{nope}}".to_owned();
        assert!(resolve(&t, &[]).unwrap_err().contains("nope"));
        t.url = "http://a/{{x}}".to_owned();
        assert!(
            resolve(&t, &pairs(&[("x", "{{x}}")]))
                .unwrap_err()
                .contains("itself")
        );
    }

    #[test]
    fn nothing_can_be_smuggled_and_errors_do_not_show_values() {
        let secret = "hunter2\r\nEvil: 1";
        let mut t = at(3);
        t.url = "http://a".to_owned();
        t.headers = pairs(&[("Authorization", "{{token}}")]);
        let err = resolve(&t, &pairs(&[("token", secret)])).unwrap_err();
        assert!(!err.contains("hunter2"), "{err}");
        t.headers = pairs(&[("@/etc/passwd", "x")]);
        assert!(resolve(&t, &[]).is_err(), "curl would read this file");
        t.headers.clear();
        for url in ["file:///etc/passwd", "gopher://a", "http://a b", "{{u}}"] {
            t.url = url.to_owned();
            assert!(resolve(&t, &pairs(&[("u", "file:///x")])).is_err(), "{url}");
        }
    }

    #[test]
    fn the_curl_config_quotes_and_never_reads_files() {
        let request = HttpRequest {
            method: "POST".to_owned(),
            url: "http://a/\"b".to_owned(),
            headers: pairs(&[("X-Empty", ""), ("A", "b\\c")]),
            body: Some("@/etc/passwd\nline".to_owned()),
        };
        let config = curl_config(&request);
        assert!(config.contains("url = \"http://a/\\\"b\"\n"), "{config}");
        assert!(config.contains("header = \"X-Empty;\"\n"), "{config}");
        assert!(config.contains("header = \"A: b\\\\c\"\n"), "{config}");
        assert!(
            config.contains("data-raw = \"@/etc/passwd\\nline\"\n"),
            "{config}"
        );
        assert!(config.contains("header = \"Content-Type:\"\n"), "{config}");
        let head = HttpRequest {
            method: "HEAD".to_owned(),
            url: "http://a".to_owned(),
            headers: vec![],
            body: None,
        };
        assert_eq!(curl_config(&head), "url = \"http://a\"\nhead\n");
    }

    #[test]
    fn env_files_are_name_value_lines() {
        assert_eq!(
            parse_env("# c\nA=1\n B = x=y \nnothing\n=v\n"),
            pairs(&[("A", "1"), ("B", "x=y")])
        );
    }

    #[test]
    fn an_answer_skips_interim_blocks() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nA: b\r\n\r\nline1\nline2";
        let r = parse_response(raw, false, 7).unwrap();
        assert_eq!((r.code, r.status.as_str()), (201, "HTTP/1.1 201 Created"));
        assert_eq!(r.headers, vec!["A: b".to_owned()]);
        assert_eq!(r.body, vec!["line1".to_owned(), "line2".to_owned()]);
        assert_eq!((r.body_bytes, r.elapsed_ms), (11, 7));
        let proxied = b"HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 404 \r\n\r\n";
        assert_eq!(parse_response(proxied, false, 0).unwrap().code, 404);
        assert!(parse_response(b"garbage", false, 0).is_err());
    }

    #[test]
    fn hostile_bytes_never_reach_the_screen() {
        let raw = b"HTTP/1.1 200 \x1b]0;pwn\x07OK\r\nX: \x1b[31mred\r\n\r\n\x1b[2Jtext\x1b]52;c;AAAA\x07\x9b\x07 end\n";
        let r = parse_response(raw, false, 0).unwrap();
        let all = format!("{}{:?}{:?}", r.status, r.headers, r.body);
        assert!(
            !all.contains('\u{1b}') && !all.contains('\u{7}') && !all.contains("AAAA"),
            "{all}"
        );
        assert!(r.body[0].contains("text"), "{:?}", r.body);
    }

    #[test]
    fn a_binary_body_is_not_shown() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\n\r\nPNG\0\x01\x02", false, 0).unwrap();
        assert!(r.binary && r.body.is_empty());
        let panel = HttpPanel {
            outcome: Some(HttpOutcome::Done(r)),
            ..HttpPanel::default()
        };
        assert!(panel.rows().iter().any(|(_, t)| t.contains("Binary")));
    }
}

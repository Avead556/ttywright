use super::*;
use crate::core::http::{HttpEffect, HttpEvent, HttpOutcome, HttpResponse};

const FILE: &str = "GET http://a/one\n\n###\nPOST http://a/{{two}}\n\nbody\n";

fn sent(fx: &[Effect]) -> Option<(u64, String)> {
    fx.iter().rev().find_map(|e| match e {
        Effect::Http(HttpEffect::Send { id, template, .. }) => Some((*id, template.title())),
        _ => None,
    })
}

fn response(code: u16) -> HttpOutcome {
    HttpOutcome::Done(HttpResponse {
        status: format!("HTTP/1.1 {code} X"),
        code,
        headers: vec!["A: b".to_owned()],
        body: vec!["hello".to_owned()],
        body_bytes: 5,
        binary: false,
        truncated: false,
        elapsed_ms: 3,
    })
}

#[test]
fn opening_a_file_sends_nothing_and_the_action_sends_the_block_under_the_cursor() {
    let mut h = Harness::new(&[("a.http", FILE)]);
    h.open("a.http");
    assert_eq!(h.ws.http_panel().current, None, "opening sends nothing");
    for _ in 0..4 {
        h.run(Command::Move(Motion::Down));
    }
    let fx = h.ws.dispatch(Command::Run(Action::HttpRun));
    let (_, title) = sent(&fx).expect("sent");
    assert_eq!(title, "POST http://a/{{two}}", "the variable stays a name");
    assert_eq!(h.ws.tool_tab(), ToolTab::Http);
    assert!(h.ws.tool_window_visible());
    assert_eq!(h.ws.focus(), Focus::Editor, "typing goes on");
}

#[test]
fn other_files_and_empty_blocks_send_nothing() {
    let mut h = Harness::new(&[("a.rs", "GET http://a\n"), ("b.http", "# nothing\n")]);
    h.open("a.rs");
    assert!(sent(&h.ws.dispatch(Command::Run(Action::HttpRun)).clone()).is_none());
    assert!(status_text(&h).contains(".http"), "{}", status_text(&h));
    h.open("b.http");
    assert!(sent(&h.ws.dispatch(Command::Run(Action::HttpRun)).clone()).is_none());
    assert!(
        status_text(&h).contains("No request"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn the_answer_is_shown_and_an_old_one_is_ignored_after_a_new_request() {
    let mut h = Harness::new(&[("a.http", FILE)]);
    h.open("a.http");
    let first = sent(&h.ws.dispatch(Command::Run(Action::HttpRun)).clone())
        .unwrap()
        .0;
    let again = h.ws.dispatch(Command::Run(Action::HttpRun));
    assert!(
        again
            .iter()
            .any(|e| matches!(e, Effect::Http(HttpEffect::Cancel { id }) if *id == first))
    );
    let second = sent(&again).unwrap().0;
    h.ws.handle(Event::Http(HttpEvent::Finished {
        id: first,
        outcome: response(500),
    }));
    assert!(h.ws.http_panel().outcome.is_none());
    assert_eq!(h.ws.http_panel().current, Some(second));
    h.ws.handle(Event::Http(HttpEvent::Finished {
        id: second,
        outcome: response(200),
    }));
    assert_eq!(h.ws.http_panel().current, None);
    assert!(
        h.ws.http_panel()
            .rows()
            .iter()
            .any(|(_, t)| t.contains("hello"))
    );
}

#[test]
fn stop_cancels_what_is_running_and_the_panel_scrolls() {
    let mut h = Harness::new(&[("a.http", FILE)]);
    h.open("a.http");
    let id = sent(&h.ws.dispatch(Command::Run(Action::HttpRun)).clone())
        .unwrap()
        .0;
    h.act(Action::ShowHttp);
    assert_eq!(h.ws.focus(), Focus::Http);
    let fx = h.ws.dispatch(Command::Insert("s".to_owned()));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Http(HttpEffect::Cancel { id: i }) if *i == id))
    );
    h.ws.handle(Event::Http(HttpEvent::Finished {
        id,
        outcome: response(200),
    }));
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.ws.http_panel().scroll, 1);
    h.run(Command::ScrollHttp { delta: -5 });
    assert_eq!(h.ws.http_panel().scroll, 0);
    h.run(Command::ScrollHttp { delta: 1000 });
    assert!(h.ws.http_panel().scroll < h.ws.http_panel().rows().len());
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn a_broken_block_says_why_and_sends_nothing() {
    let mut h = Harness::new(&[("a.http", "GET http://a\nnot a header\n")]);
    h.open("a.http");
    assert!(sent(&h.ws.dispatch(Command::Run(Action::HttpRun)).clone()).is_none());
    assert!(
        status_text(&h).contains("not a header"),
        "{}",
        status_text(&h)
    );
}

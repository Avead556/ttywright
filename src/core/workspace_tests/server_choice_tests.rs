use super::*;

#[test]
fn choosing_a_server_lists_them_marks_the_current_and_sends_the_choice() {
    let mut h = Harness::new(&[]);
    h.act(Action::SelectLanguageServer);
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::ListServers)));
    assert!(h.ws.picker().is_none(), "the list is still loading");
    h.lsp_event(Event::Lsp(LspEvent::ServersListed {
        options: server_options(),
    }));
    let kind = h.ws.picker().expect("a list").kind;
    assert_eq!(h.ws.picker_items(kind), ["● Automatic", "  2025-12-08"]);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert!(h.ws.picker().is_none());
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::SelectServer(
        ServerChoice::Release("2025-12-08".into())
    ))));
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Language server: 2025-12-08")
    );
}

#[test]
fn a_late_server_list_does_not_replace_another_list() {
    let mut h = Harness::new(&[]);
    h.act(Action::SelectLanguageServer);
    h.act(Action::OpenPalette);
    h.lsp_event(Event::Lsp(LspEvent::ServersListed {
        options: server_options(),
    }));
    assert_eq!(h.ws.picker().map(|p| p.kind), Some(PickerKind::Actions));
}

#[test]
fn escape_leaves_the_server_as_it_was() {
    let mut h = Harness::new(&[]);
    h.act(Action::SelectLanguageServer);
    h.lsp_event(Event::Lsp(LspEvent::ServersListed {
        options: server_options(),
    }));
    h.run(Command::Escape);
    assert!(
        !h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::SelectServer(_))))
    );
}

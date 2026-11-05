use super::*;

#[test]
fn each_language_group_starts_its_own_server_when_first_needed() {
    let mut h = with_lang_files(&[("a.py", "x"), ("n.txt", "x")]);
    assert_eq!(sessions_started(&h), [server_at(Language::Python, ROOT)]);
    h.run(Command::OpenFile {
        path: Path::new(ROOT).join("b.py"),
        at: None,
    });
    assert_eq!(
        sessions_started(&h).len(),
        1,
        "the same session takes the next file"
    );
    let mut h = with_lang_files(&[("a.js", ""), ("b.ts", ""), ("c.tsx", ""), ("d.jsx", "")]);
    assert_eq!(
        sessions_started(&h),
        [server_at(Language::TypeScript, ROOT)],
        "JavaScript and TypeScript share one"
    );
    ready(&mut h, server_at(Language::TypeScript, ROOT));
    let languages: Vec<_> = h
        .lsp
        .iter()
        .filter_map(|e| match e {
            Effect::Lsp(LspEffect::DidOpen { language, .. }) => Some(*language),
            _ => None,
        })
        .collect();
    assert_eq!(languages.len(), 4);
    assert!(languages.contains(&Language::Jsx) && languages.contains(&Language::Tsx));
}

#[test]
fn files_without_a_server_start_nothing() {
    let h = with_lang_files(&[("a.toml", ""), ("b.md", ""), ("c.json", ""), ("d.txt", "")]);
    assert!(h.lsp.is_empty(), "{:?}", h.lsp);
}

#[test]
fn nested_projects_get_a_session_each_and_documents_go_to_their_own() {
    let mut h = with_lang_files(&[
        ("one/go.mod", ""),
        ("two/go.mod", ""),
        ("one/a.go", "a"),
        ("two/b.go", "b"),
        ("c.go", "c"),
    ]);
    let (one, two) = (
        server_at(Language::Go, "/proj/one"),
        server_at(Language::Go, "/proj/two"),
    );
    let top = server_at(Language::Go, ROOT);
    let mut keys = sessions_started(&h);
    keys.sort();
    assert_eq!(keys, [top, one.clone(), two.clone()]);
    ready(&mut h, one.clone());
    let opened: Vec<_> = h
        .lsp
        .iter()
        .filter_map(|e| match e {
            Effect::Lsp(LspEffect::DidOpen { server, path, .. }) => {
                Some((server.clone(), path.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(opened, [(one, PathBuf::from("/proj/one/a.go"))]);
}

#[test]
fn state_queries_and_restart_follow_the_active_document() {
    let mut h = with_lang_files(&[("a.rs", "fn"), ("b.py", "x")]);
    let (rust, python) = (rust_server(), server_at(Language::Python, ROOT));
    assert_eq!(h.ws.lsp_status().unwrap().name, "pyright-langserver");
    ready(&mut h, rust.clone());
    assert_eq!(
        h.ws.lsp_state(),
        LspState::Starting,
        "Rust's being up says nothing"
    );
    h.act(Action::GotoDefinition);
    assert!(
        !h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::Query { .. })))
    );
    ready(&mut h, python.clone());
    assert_eq!(h.ws.lsp_state(), LspState::Ready);
    h.act(Action::GotoDefinition);
    assert!(h.lsp.iter().any(|e| matches!(
        e,
        Effect::Lsp(LspEffect::Query { server, path, .. }) if *server == python && path.ends_with("b.py")
    )));
    h.act(Action::RestartLanguageServer);
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::Restart {
        server: python.clone()
    })));
    assert!(
        !h.lsp
            .contains(&Effect::Lsp(LspEffect::Restart { server: rust }))
    );
}

#[test]
fn a_crash_leaves_other_sessions_their_diagnostics_and_questions() {
    let mut h = with_lang_files(&[("a.rs", "fn"), ("b.py", "x")]);
    let (rust, python) = (rust_server(), server_at(Language::Python, ROOT));
    ready(&mut h, rust.clone());
    ready(&mut h, python.clone());
    let diagnostic = |path: &str, server: ServerKey| {
        Event::Lsp(LspEvent::Diagnostics {
            server,
            path: PathBuf::from(path),
            items: vec![RawDiagnostic {
                severity: Severity::Error,
                message: "bad".to_owned(),
                start: (0, 0),
                end: (0, 1),
            }],
        })
    };
    h.lsp_event(diagnostic("/proj/a.rs", rust.clone()));
    h.lsp_event(diagnostic("/proj/b.py", python.clone()));
    assert_eq!(h.ws.problems().len(), 2);
    h.act(Action::GotoDefinition);
    let asked = h.lsp.iter().rev().find_map(|e| match e {
        Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
        _ => None,
    });
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: rust.clone(),
        state: LspState::Crashed,
    }));
    assert_eq!(
        h.ws.lsp_state(),
        LspState::Ready,
        "python is active and fine"
    );
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: python.clone(),
        id: asked.unwrap(),
        result: Ok(LspAnswer::Hover(Some("from python".to_owned()))),
    }));
    assert!(h.ws.hover().is_some(), "a crash elsewhere must not lose it");
    ready(&mut h, rust);
    let left: Vec<_> = h.ws.problems().iter().map(|p| p.path.clone()).collect();
    assert_eq!(left, [PathBuf::from("/proj/b.py")]);
}

#[test]
fn each_session_is_asked_in_the_units_it_chose() {
    let mut h = with_lang_files(&[("a.rs", "𝄞x"), ("b.py", "𝄞x")]);
    let (rust, python) = (rust_server(), server_at(Language::Python, ROOT));
    ready(&mut h, rust.clone());
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: python.clone(),
        encoding: PositionEncoding::Utf16,
        completion_triggers: Vec::new(),
    }));
    let ask = |h: &mut Harness| {
        h.run(Command::Move(Motion::Right));
        h.lsp.clear();
        h.act(Action::GotoDefinition);
        h.lsp.iter().find_map(|e| match e {
            Effect::Lsp(LspEffect::Query {
                query: LspQuery::Definition { at },
                server,
                ..
            }) => Some((server.clone(), *at)),
            _ => None,
        })
    };
    assert_eq!(
        ask(&mut h),
        Some((python.clone(), (0, 2))),
        "two UTF-16 units"
    );
    h.act(Action::NextTab);
    assert_eq!(ask(&mut h), Some((rust, (0, 4))), "four UTF-8 bytes");
}

#[test]
fn a_utf16_servers_answers_are_read_in_utf16() {
    let mut h = with_lang_files(&[("b.py", "𝄞ы\n𝄞x\n")]);
    let python = server_at(Language::Python, ROOT);
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: python.clone(),
        encoding: PositionEncoding::Utf16,
        completion_triggers: Vec::new(),
    }));
    h.act(Action::GotoDefinition);
    let id = h.lsp.iter().find_map(|e| match e {
        Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
        _ => None,
    });
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: python.clone(),
        id: id.unwrap(),
        result: Ok(LspAnswer::Locations(vec![LspLocation {
            path: PathBuf::from("/proj/b.py"),
            start: (1, 2),
        }])),
    }));
    let (view, d) = h.ws.active().unwrap();
    assert_eq!(view.cursor.idx, d.text().line_to_char(1) + 1, "after the 𝄞");
    h.lsp_event(Event::Lsp(LspEvent::Diagnostics {
        server: python,
        path: PathBuf::from("/proj/b.py"),
        items: vec![RawDiagnostic {
            severity: Severity::Error,
            message: "bad".to_owned(),
            start: (1, 2),
            end: (1, 3),
        }],
    }));
    let problems = h.ws.problems();
    assert_eq!(problems[0].col, 1, "the character after the 𝄞");
}

#[test]
fn a_session_turned_off_says_nothing_and_cannot_be_asked() {
    let mut h = with_lang_files(&[("a.go", "x")]);
    let go = server_at(Language::Go, ROOT);
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: go,
        state: LspState::Disabled,
    }));
    h.lsp.clear();
    h.act(Action::GotoDefinition);
    assert_eq!(status_text(&h), "No language server for this file");
    h.act(Action::RestartLanguageServer);
    assert_eq!(status_text(&h), "No language server is running");
    assert!(h.lsp.is_empty(), "{:?}", h.lsp);
}

#[test]
fn a_missing_server_tells_what_to_install_only_for_what_the_table_knows() {
    let mut h = with_lang_files(&[("a.go", "x")]);
    let go = server_at(Language::Go, ROOT);
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: go.clone(),
        state: LspState::NotFound,
    }));
    let status = h.ws.lsp_status().unwrap();
    assert_eq!(status.name, "gopls");
    assert_eq!(
        status.install,
        Some("go install golang.org/x/tools/gopls@latest")
    );
    h.lsp_event(Event::Lsp(LspEvent::ServerNamed {
        server: go,
        name: "my-gopls".to_owned(),
    }));
    let status = h.ws.lsp_status().unwrap();
    assert_eq!(status.name, "my-gopls");
    assert_eq!(status.install, None);
}

#[test]
fn rust_never_offers_an_install_command_as_it_fetches_its_own() {
    let mut h = with_lang_files(&[("a.rs", "x")]);
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::NotFound,
    }));
    assert_eq!(h.ws.lsp_status().unwrap().install, None);
}

#[test]
fn select_language_server_stays_with_rust() {
    let mut h = with_lang_files(&[("a.py", "x")]);
    h.lsp.clear();
    h.act(Action::SelectLanguageServer);
    assert!(!h.lsp.contains(&Effect::Lsp(LspEffect::ListServers)));
    assert!(
        status_text(&h).contains("[lsp.python]"),
        "{}",
        status_text(&h)
    );
    let mut h = with_lang_files(&[("a.rs", "x")]);
    h.lsp.clear();
    h.act(Action::SelectLanguageServer);
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::ListServers)));
}

#[test]
fn a_script_with_only_a_shebang_gets_its_languages_server() {
    let h = with_lang_files(&[("deploy", "#!/usr/bin/env bash\necho\n")]);
    assert_eq!(sessions_started(&h), [server_at(Language::Bash, ROOT)]);
    let (_, d) = h.ws.active().unwrap();
    assert_eq!(d.language(), Language::Bash);
    assert!(d.syntax().is_some());
}

#[test]
fn typing_a_shebang_moves_a_new_file_to_that_languages_server() {
    let mut h = with_lang_files(&[("tool", "")]);
    assert!(sessions_started(&h).is_empty());
    h.run(Command::Insert("#!/usr/bin/python3\n".to_owned()));
    let python = server_at(Language::Python, ROOT);
    assert_eq!(sessions_started(&h), std::slice::from_ref(&python));
    ready(&mut h, python.clone());
    assert!(h.lsp.iter().any(|e| matches!(
        e,
        Effect::Lsp(LspEffect::DidOpen { server, language: Language::Python, .. }) if *server == python
    )));
    h.lsp.clear();
    h.act(Action::SelectAll);
    h.run(Command::Insert("not a script\n".to_owned()));
    assert!(
        h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::DidClose { .. })))
    );
    assert_eq!(h.ws.active().unwrap().1.language(), Language::PlainText);
}

#[test]
fn a_library_file_stays_with_the_session_that_pointed_there() {
    let mut h = with_lang_files(&[
        ("one/go.mod", ""),
        ("two/go.mod", ""),
        ("one/a.go", "a"),
        ("two/b.go", "b"),
    ]);
    let one = server_at(Language::Go, "/proj/one");
    let two = server_at(Language::Go, "/proj/two");
    ready(&mut h, one);
    ready(&mut h, two.clone());
    h.act(Action::GotoDefinition);
    let id = h.lsp.iter().find_map(|e| match e {
        Effect::Lsp(LspEffect::Query { id, server, .. }) if *server == two => Some(*id),
        _ => None,
    });
    h.files
        .push((PathBuf::from("/lib/fmt.go"), "package fmt".to_owned()));
    let before = sessions_started(&h).len();
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: two.clone(),
        id: id.unwrap(),
        result: Ok(LspAnswer::Locations(vec![LspLocation {
            path: PathBuf::from("/lib/fmt.go"),
            start: (0, 0),
        }])),
    }));
    assert_eq!(
        sessions_started(&h).len(),
        before,
        "no session rooted in a library"
    );
    let (_, d) = h.ws.active().unwrap();
    assert_eq!(d.lsp_server(), Some(&two));
}

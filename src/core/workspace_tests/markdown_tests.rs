use super::*;
use crate::core::image_art::{ImageRequest, ImageState};

fn harness_with(files: &[(&str, &[u8])]) -> Harness {
    let texts: Vec<(&str, &str)> = files.iter().map(|(n, _)| (*n, "")).collect();
    let mut h = Harness::new(&texts);
    h.ws.configure_markdown(true);
    h
}

fn open_bytes(h: &mut Harness, name: &str, bytes: &[u8]) {
    let path = Path::new(ROOT).join(name);
    h.lsp_event(Event::FileRead {
        path,
        at: None,
        disk: None,
        result: Ok(Some(bytes.to_vec())),
    });
}

fn active(h: &Harness) -> DocumentId {
    h.ws.view(
        h.ws.pane(h.ws.active_pane())
            .unwrap()
            .active_view()
            .unwrap(),
    )
    .unwrap()
    .doc
}

#[test]
fn a_markdown_file_opens_with_its_preview_and_others_do_not() {
    let mut h = Harness::new(&[("a.md", "# t"), ("b.txt", "x"), ("c.markdown", "# t")]);
    h.open("a.md");
    assert!(h.ws.markdown_preview(active(&h)));
    h.open("c.markdown");
    assert!(h.ws.markdown_preview(active(&h)));
    h.open("b.txt");
    assert!(!h.ws.markdown_preview(active(&h)));
}

#[test]
fn the_setting_turns_the_default_off_and_the_action_still_works() {
    let mut h = Harness::new(&[("a.md", "# t")]);
    h.ws.configure_markdown(false);
    h.open("a.md");
    assert!(!h.ws.markdown_preview(active(&h)));
    h.act(Action::MarkdownPreview);
    assert!(h.ws.markdown_preview(active(&h)));
}

#[test]
fn a_files_own_choice_survives_a_restart() {
    let mut h = Harness::new(&[("a.md", "# t"), ("b.md", "# t")]);
    h.open("b.md");
    h.open("a.md");
    h.act(Action::MarkdownPreview);
    assert!(!h.ws.markdown_preview(active(&h)), "closed by hand");
    let saved = h.ws.dispatch(Command::SaveSession);
    let session = saved
        .into_iter()
        .find_map(|e| match e {
            Effect::WriteSession(s) => Some(s),
            _ => None,
        })
        .expect("a session is written");
    assert_eq!(
        session.markdown_preview.get(&Path::new(ROOT).join("a.md")),
        Some(&false)
    );
    let mut again = Harness::new(&[("a.md", "# t"), ("b.md", "# t")]);
    again.ws.handle(Event::SessionLoaded {
        session,
        files: Vec::new(),
    });
    again.open("a.md");
    assert!(!again.ws.markdown_preview(active(&again)));
    again.open("b.md");
    assert!(again.ws.markdown_preview(active(&again)));
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest of it";

#[test]
fn a_picture_opens_read_only_and_nothing_can_write_it() {
    let mut h = harness_with(&[]);
    open_bytes(&mut h, "a.png", PNG);
    let doc = active(&h);
    let d = h.ws.document(doc).unwrap();
    assert!(d.is_image());
    assert_eq!(d.text().len_chars(), 0, "no text to show as garbage");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    h.run(Command::Insert("x".to_owned()));
    let d = h.ws.document(doc).unwrap();
    assert_eq!(d.text().len_chars(), 0);
    assert!(!d.is_dirty());
    assert!(
        status_text(&h).contains("cannot be edited"),
        "{}",
        status_text(&h)
    );
    h.act(Action::Save);
    assert!(h.written.is_empty(), "never written");
    h.lsp_event(Event::FileChanged {
        doc,
        disk: None,
        result: Ok(Some(PNG.to_vec())),
    });
    assert!(h.ws.document(doc).unwrap().conflict().is_none());
}

#[test]
fn a_picture_is_asked_for_once_and_rechecked_when_the_disk_may_have_changed() {
    let mut h = harness_with(&[]);
    let request = ImageRequest {
        dir: PathBuf::from(ROOT),
        source: "a.png".to_owned(),
        cols: 10,
        rows: 10,
    };
    let asked = |fx: &[Effect]| {
        fx.iter()
            .filter(|e| matches!(e, Effect::DecodeImage { .. }))
            .count()
    };
    let first = h.ws.dispatch(Command::WantImage(request.clone()));
    assert_eq!(asked(&first), 1);
    assert!(asked(&h.ws.dispatch(Command::WantImage(request.clone()))) == 0);
    assert_eq!(h.ws.image_state(&request), Some(&ImageState::Decoding));
    h.ws.handle(Event::ImageDecoded {
        request: request.clone(),
        result: Err("the file is damaged".to_owned()),
    });
    assert_eq!(
        h.ws.image_state(&request),
        Some(&ImageState::Failed("the file is damaged".to_owned()))
    );
    assert_eq!(asked(&h.ws.dispatch(Command::CheckDisk)), 1);
    let changed = h.ws.handle(Event::PathsChanged {
        paths: vec![Path::new(ROOT).join("a.png")],
    });
    assert_eq!(asked(&changed), 1);
    let unrelated = h.ws.handle(Event::PathsChanged {
        paths: vec![Path::new(ROOT).join("a.txt")],
    });
    assert_eq!(asked(&unrelated), 0);
}

#[test]
fn a_screen_full_of_requests_stays_bounded() {
    let mut h = harness_with(&[]);
    let at = |i: usize| ImageRequest {
        dir: PathBuf::from(ROOT),
        source: format!("{i}.png"),
        cols: 10,
        rows: 10,
    };
    for i in 0..200 {
        h.ws.dispatch(Command::WantImage(at(i)));
    }
    assert!(h.ws.image_state(&at(0)).is_none(), "old ones are dropped");
    assert!(h.ws.image_state(&at(199)).is_some());
    let silly = ImageRequest {
        cols: 60_000,
        ..at(1)
    };
    assert!(h.ws.dispatch(Command::WantImage(silly)).is_empty());
}

#[test]
fn keys_zoom_and_move_a_picture_and_stay_inside_it() {
    use crate::core::image_art::{ArtCell, AsciiImage};
    let mut h = harness_with(&[]);
    open_bytes(&mut h, "a.png", PNG);
    let doc = active(&h);
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let source = h.ws.document(doc).unwrap().path().display().to_string();
    let request = ImageRequest {
        dir: PathBuf::from(ROOT),
        source,
        cols: 20,
        rows: 10,
    };
    h.ws.dispatch(Command::WantImage(request.clone()));
    h.lsp_event(Event::ImageDecoded {
        request,
        result: Ok(AsciiImage {
            cols: 20,
            rows: 10,
            cells: vec![
                ArtCell {
                    ch: ' ',
                    dots: 0,
                    ink: None,
                    paper: None
                };
                200
            ],
            width: 40,
            height: 40,
        }),
    });
    let view = |h: &Harness| h.ws.picture_view(doc);
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Down));
    assert_eq!((view(&h).x, view(&h).y), (8, 4));
    h.run(Command::Insert("+".to_owned()));
    assert_eq!(
        (view(&h).zoom, view(&h).x, view(&h).y),
        (1, 16, 8),
        "same place"
    );
    for _ in 0..9 {
        h.run(Command::Move(Motion::Right));
        h.run(Command::Move(Motion::Down));
    }
    assert_eq!((view(&h).x, view(&h).y), (19, 9), "not past the picture");
    for _ in 0..9 {
        h.run(Command::Insert("=".to_owned()));
    }
    assert_eq!(view(&h).zoom, crate::core::image_art::MAX_ZOOM);
    h.run(Command::Insert("-".to_owned()));
    assert_eq!(view(&h).zoom, crate::core::image_art::MAX_ZOOM - 1);
    h.run(Command::Insert("0".to_owned()));
    assert_eq!(view(&h), Default::default());
    assert!(!h.ws.document(doc).unwrap().is_dirty());
}

fn opened_source(
    h: &mut Harness,
    act: impl FnOnce(&mut Workspace) -> Vec<Effect>,
) -> Option<String> {
    act(&mut h.ws).into_iter().find_map(|e| match e {
        Effect::OpenImage { request, .. } => Some(request.source),
        _ => None,
    })
}

#[test]
fn o_sends_a_picture_tab_outside_and_changes_nothing() {
    let mut h = harness_with(&[]);
    open_bytes(&mut h, "a.png", PNG);
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let source = opened_source(&mut h, |ws| ws.dispatch(Command::Insert("o".to_owned())));
    assert!(source.is_some_and(|s| s.ends_with("a.png")));
    assert!(status_text(&h).is_empty() || !status_text(&h).contains("edited"));
}

#[test]
fn in_markdown_it_is_the_picture_on_the_cursors_line() {
    let mut h = Harness::new(&[(
        "a.md",
        "text\n![cat](img/cat.png \"t\")\n![web](https://x/a.png)\n",
    )]);
    h.open("a.md");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let open = |ws: &mut Workspace| ws.dispatch(Command::Run(Action::OpenInSystemViewer));
    assert_eq!(opened_source(&mut h, open), None, "no picture here");
    assert!(
        status_text(&h).contains("Put the cursor"),
        "{}",
        status_text(&h)
    );
    h.run(Command::Move(Motion::Down));
    assert_eq!(opened_source(&mut h, open).as_deref(), Some("img/cat.png"));
    h.run(Command::Move(Motion::Down));
    assert_eq!(
        opened_source(&mut h, open),
        None,
        "nothing goes to the network"
    );
    assert!(
        status_text(&h).contains("not a local file"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn an_o_in_ordinary_text_is_still_a_letter() {
    let mut h = Harness::new(&[("a.txt", "")]);
    h.open("a.txt");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    h.run(Command::Insert("o".to_owned()));
    assert_eq!(h.text_of("a.txt").as_deref(), Some("o"));
}

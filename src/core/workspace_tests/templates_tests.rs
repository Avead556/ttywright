use super::*;

#[test]
fn a_file_from_a_template_goes_beside_the_open_one_with_its_contents() {
    let mut h = Harness::new(&[("src/a.php", "x")]);
    h.open("src/a.php");
    h.act(Action::NewFromTemplate);
    assert_eq!(
        h.ws.picker().map(|p| p.kind),
        Some(PickerKind::FileTemplates)
    );
    h.run(Command::SelectPickerItem {
        index: template_index("PHP class"),
        accept: true,
    });
    assert!(h.ws.picker().is_none());
    h.typed_prompt("UserRepo");
    h.run(Command::Newline);
    let Some(crate::core::file_op::FileOp::Create {
        path,
        contents,
        executable,
    }) = h.last_file_op()
    else {
        panic!("{:?}", h.last_file_op());
    };
    assert_eq!(path, Path::new(ROOT).join("src/UserRepo.php"));
    assert!(contents.contains("final class UserRepo"), "{contents}");
    assert!(!executable);
    h.files.push((path.clone(), contents.clone()));
    let op = h.last_file_op().unwrap();
    h.lsp_event(Event::FileOpDone { op, result: Ok(()) });
    assert!(h.active_text().contains("final class UserRepo"));
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn a_template_with_a_fixed_name_offers_it_and_a_taken_name_stays_in_the_prompt() {
    let mut h = Harness::new(&[]);
    h.act(Action::NewFromTemplate);
    h.run(Command::SelectPickerItem {
        index: template_index(".gitignore: Rust"),
        accept: true,
    });
    assert_eq!(
        h.ws.file_prompt()
            .map(|p| p.input.text().to_owned())
            .as_deref(),
        Some(".gitignore")
    );
    h.run(Command::Newline);
    let op = h.last_file_op().expect("asked");
    h.lsp_event(Event::FileOpDone {
        op,
        result: Err("File exists".into()),
    });
    assert_eq!(
        h.ws.file_prompt().and_then(|p| p.error.clone()).as_deref(),
        Some("File exists")
    );
}

#[test]
fn a_new_project_goes_beside_this_one_named_after_its_folder_and_opens_when_made() {
    let mut h = Harness::new(&[]);
    h.act(Action::NewProject);
    let index = crate::core::templates::PROJECT_TEMPLATES
        .iter()
        .position(|t| t.label == "Rust: application")
        .unwrap();
    h.run(Command::SelectPickerItem {
        index,
        accept: true,
    });
    let suggested =
        h.ws.file_prompt()
            .map(|p| p.input.text().to_owned())
            .unwrap();
    assert!(suggested.ends_with("/my-project"), "{suggested}");
    for _ in 0..suggested.chars().count() {
        h.run(Command::DeleteBackward);
    }
    h.typed_prompt("Cool App");
    h.run(Command::Newline);
    let op = h.last_file_op().expect("asked");
    let crate::core::file_op::FileOp::CreateProject { dir, files } = &op else {
        panic!("{op:?}");
    };
    assert_eq!(dir, &Path::new(ROOT).parent().unwrap().join("Cool App"));
    let cargo = files.iter().find(|(p, _)| p == "Cargo.toml").unwrap();
    assert!(cargo.1.contains("name = \"cool-app\""));
    assert!(
        !h.git
            .iter()
            .any(|e| matches!(e, Effect::OpenProject { .. }))
    );
    h.lsp_event(Event::FileOpDone { op, result: Ok(()) });
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::OpenProject { .. }))
    );
    assert!(h.ws.file_prompt().is_none());
}

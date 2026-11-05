use super::*;

#[test]
fn no_command_from_a_plugin_can_panic_whatever_is_on_screen() {
    for action in Action::ALL.iter().copied().filter(|a| *a != Action::Quit) {
        let mut h = Harness::new(&[("a.rs", "abc\ndef\n")]);
        h.open("a.rs");
        drop(h.ws.dispatch(Command::Run(action)));
        let real = h.ws.active_view();
        for command in hostile_commands() {
            drop(h.ws.dispatch(command));
        }
        if let Some(view) = real {
            let far = usize::MAX;
            drop(h.ws.dispatch(Command::FocusView {
                view,
                at: Some((far, far)),
            }));
            drop(h.ws.dispatch(Command::ToggleBreakpointAt { view, line: far }));
        }
    }
}

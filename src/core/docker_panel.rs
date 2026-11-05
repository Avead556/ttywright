use super::cli::{CliEffect, CliOutcome, CliTool, PanelCx, PanelReply};
use super::command::{Command, Motion};
use super::docker::{
    DockerConfirm, DockerPanel, DockerRequest, DockerSection, parse_containers, parse_images,
    parse_services, plain_failure,
};
use super::run::{RunCommand, RunSource, RunTarget};

const MISSING: &str = "docker is not installed";
const COMPOSE_WHY: &str = "A compose file can run anything: this project is not trusted";

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| (*w).to_owned()).collect()
}

impl DockerPanel {
    pub fn refresh(&mut self, cx: &PanelCx, out: &mut Vec<PanelReply>) {
        if self.pending.any(|r| r != DockerRequest::Action) {
            return;
        }
        self.daemon_note = None;
        if cx.trusted {
            self.compose_note = None;
            let args = words(&["compose", "config", "--format", "json", "--no-interpolate"]);
            self.ask(DockerRequest::Services, args, true, cx, out);
        } else {
            self.services.clear();
            self.compose_note = Some(COMPOSE_WHY.to_owned());
        }
        let args = words(&["ps", "-a", "--format", "json"]);
        self.ask(DockerRequest::Containers, args, false, cx, out);
        let args = words(&["images", "--format", "json"]);
        self.ask(DockerRequest::Images, args, false, cx, out);
    }

    fn ask(
        &mut self,
        request: DockerRequest,
        args: Vec<String>,
        in_project: bool,
        cx: &PanelCx,
        out: &mut Vec<PanelReply>,
    ) {
        let id = self.pending.ask(request);
        out.push(PanelReply::Cli(CliEffect::Run {
            id,
            tool: CliTool::Docker,
            root: cx.root.clone(),
            args,
            input: None,
            in_project,
            write: request == DockerRequest::Action,
        }));
    }

    pub fn answered(
        &mut self,
        id: u64,
        outcome: CliOutcome,
        cx: &PanelCx,
        out: &mut Vec<PanelReply>,
    ) {
        let Some(request) = self.pending.take(id) else {
            return;
        };
        let failure = match outcome {
            CliOutcome::Done(text) => return self.read(request, &text, cx, out),
            CliOutcome::Missing => MISSING.to_owned(),
            CliOutcome::Failed(text) => plain_failure(&text),
            CliOutcome::TimedOut => "docker did not answer in time".to_owned(),
            CliOutcome::TooLong => "docker printed too much".to_owned(),
        };
        match request {
            DockerRequest::Services => {
                self.services.clear();
                self.compose_note = Some(failure);
            }
            DockerRequest::Containers => {
                self.containers.clear();
                self.daemon_note = Some(failure);
            }
            DockerRequest::Images => {
                self.images.clear();
                self.daemon_note = Some(failure);
            }
            DockerRequest::Action => {
                out.push(PanelReply::Error(format!("Docker: {failure}")));
                self.refresh(cx, out);
            }
        }
    }

    fn read(
        &mut self,
        request: DockerRequest,
        text: &str,
        cx: &PanelCx,
        out: &mut Vec<PanelReply>,
    ) {
        match request {
            DockerRequest::Services => match parse_services(text) {
                Ok(services) => self.services = services,
                Err(why) => self.compose_note = Some(why),
            },
            DockerRequest::Containers => self.containers = parse_containers(text),
            DockerRequest::Images => self.images = parse_images(text),
            DockerRequest::Action => {
                out.push(PanelReply::Info("Docker: done".to_owned()));
                self.refresh(cx, out);
            }
        }
        let chosen = self.section;
        for section in [
            DockerSection::Services,
            DockerSection::Containers,
            DockerSection::Images,
        ] {
            self.section = section;
            self.step(0);
        }
        self.section = chosen;
    }

    pub fn pick(&mut self, section: usize, index: usize) {
        self.section = match section {
            0 => DockerSection::Services,
            1 => DockerSection::Containers,
            _ => DockerSection::Images,
        };
        self.selected[self.section.index()] = index;
        self.step(0);
    }

    pub fn input(&mut self, input: &Command, cx: &PanelCx, out: &mut Vec<PanelReply>) {
        let rows = |rows: usize| isize::try_from(rows).unwrap_or(isize::MAX);
        match input {
            Command::Indent => self.section = self.section.next(),
            Command::Move(Motion::Up) => self.step(-1),
            Command::Move(Motion::Down) => self.step(1),
            Command::Move(Motion::PageUp { rows: n }) => self.step(-rows(*n)),
            Command::Move(Motion::PageDown { rows: n }) => self.step(rows(*n)),
            Command::Insert(letter) if letter == "r" => self.refresh(cx, out),
            Command::Insert(letter) => match self.section {
                DockerSection::Services => self.service_key(letter, cx, out),
                DockerSection::Containers => self.container_key(letter, cx, out),
                DockerSection::Images => self.image_key(letter),
            },
            _ => {}
        }
    }

    fn service_key(&mut self, letter: &str, cx: &PanelCx, out: &mut Vec<PanelReply>) {
        if !matches!(letter, "u" | "U" | "d" | "s" | "x" | "t" | "l" | "e") {
            return;
        }
        if !cx.trusted {
            return out.push(PanelReply::AskTrust(COMPOSE_WHY.to_owned()));
        }
        let service = self.services.get(self.chosen()).map(|s| s.name.clone());
        let compose = |list: &[&str]| {
            let mut args = words(&["compose"]);
            args.extend(words(list));
            args
        };
        match (letter, service) {
            ("U", _) => out.push(run(compose(&["up", "-d"]), RunSource::Detected, cx)),
            ("d", _) => {
                self.confirm = Some(DockerConfirm {
                    args: compose(&["down"]),
                    in_project: true,
                });
            }
            ("u", Some(name)) => {
                out.push(run(compose(&["up", "-d", &name]), RunSource::Detected, cx))
            }
            ("l", Some(name)) => {
                let args = compose(&["logs", "-f", "--tail", "500", &name]);
                out.push(run(args, RunSource::Detected, cx));
            }
            ("e", Some(name)) => {
                let mut command = words(&["docker", "compose", "exec"]);
                command.extend([name.clone(), "sh".to_owned()]);
                out.push(PanelReply::Terminal {
                    title: name,
                    command,
                    project_command: true,
                });
            }
            (letter, Some(name)) => {
                let verb = match letter {
                    "s" => "start",
                    "x" => "stop",
                    _ => "restart",
                };
                self.ask(
                    DockerRequest::Action,
                    compose(&[verb, &name]),
                    true,
                    cx,
                    out,
                );
            }
            (_, None) => out.push(PanelReply::Info("No service chosen".to_owned())),
        }
    }

    fn container_key(&mut self, letter: &str, cx: &PanelCx, out: &mut Vec<PanelReply>) {
        let Some(container) = self.containers.get(self.chosen()) else {
            return;
        };
        let (id, name) = (container.id.clone(), container.name.clone());
        match letter {
            "s" | "x" | "t" => {
                let verb = match letter {
                    "s" => "start",
                    "x" => "stop",
                    _ => "restart",
                };
                self.ask(DockerRequest::Action, words(&[verb, &id]), false, cx, out);
            }
            "l" => {
                let args = words(&["logs", "-f", "--tail", "500", &id]);
                out.push(run(args, RunSource::Typed, cx));
            }
            "e" => out.push(PanelReply::Terminal {
                title: name,
                command: words(&["docker", "exec", "-it", &id, "sh"]),
                project_command: false,
            }),
            "d" => {
                self.confirm = Some(DockerConfirm {
                    args: words(&["rm", &id]),
                    in_project: false,
                });
            }
            _ => {}
        }
    }

    fn image_key(&mut self, letter: &str) {
        let Some(image) = self.images.get(self.chosen()) else {
            return;
        };
        if letter == "d" {
            self.confirm = Some(DockerConfirm {
                args: words(&["rmi", &image.id]),
                in_project: false,
            });
        }
    }

    pub fn confirm_input(&mut self, input: &Command, cx: &PanelCx, out: &mut Vec<PanelReply>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        match input {
            Command::Insert(letter) if letter == "y" || letter == "Y" => {
                self.ask(
                    DockerRequest::Action,
                    confirm.args,
                    confirm.in_project,
                    cx,
                    out,
                );
            }
            _ => out.push(PanelReply::Info("Not run".to_owned())),
        }
    }
}

fn run(args: Vec<String>, source: RunSource, cx: &PanelCx) -> PanelReply {
    let mut argv = vec!["docker".to_owned()];
    argv.extend(args);
    PanelReply::Run(RunTarget {
        name: argv.join(" "),
        command: RunCommand::Argv(argv),
        cwd: cx.root.clone(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx(trusted: bool) -> PanelCx {
        PanelCx {
            root: "/p".into(),
            trusted,
            branch: None,
        }
    }

    fn asked(out: &[PanelReply]) -> Vec<(u64, String, bool)> {
        out.iter()
            .filter_map(|r| match r {
                PanelReply::Cli(CliEffect::Run {
                    id,
                    args,
                    in_project,
                    ..
                }) => Some((*id, args[0].clone(), *in_project)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_untrusted_project_is_never_asked_about_its_compose_file() {
        let mut panel = DockerPanel::default();
        let mut out = Vec::new();
        panel.refresh(&cx(false), &mut out);
        let words: Vec<_> = asked(&out).into_iter().map(|(_, w, p)| (w, p)).collect();
        assert_eq!(
            words,
            [("ps".to_owned(), false), ("images".to_owned(), false)]
        );
        assert_eq!(panel.compose_note.as_deref(), Some(COMPOSE_WHY));

        let mut out = Vec::new();
        panel.refresh(&cx(false), &mut out);
        assert!(out.is_empty(), "one round at a time");

        let mut out = Vec::new();
        panel.section = DockerSection::Services;
        panel.input(&Command::Insert("U".to_owned()), &cx(false), &mut out);
        assert_eq!(out, [PanelReply::AskTrust(COMPOSE_WHY.to_owned())]);
    }

    #[test]
    fn a_finished_action_says_so_and_looks_again() {
        let mut panel = DockerPanel {
            containers: parse_containers(
                r#"{"ID":"abc","Names":"web-1","Image":"nginx","State":"running","Status":"Up"}"#,
            ),
            section: DockerSection::Containers,
            ..DockerPanel::default()
        };
        let mut out = Vec::new();
        panel.input(&Command::Insert("x".to_owned()), &cx(true), &mut out);
        let first = asked(&out);
        let [(id, ref verb, false)] = first[..] else {
            panic!("{out:?}");
        };
        assert_eq!(verb, "stop");

        let mut out = Vec::new();
        panel.answered(id, CliOutcome::Done(String::new()), &cx(true), &mut out);
        assert_eq!(out[0], PanelReply::Info("Docker: done".to_owned()));
        assert_eq!(asked(&out).len(), 3, "compose, containers and images again");

        let mut again = Vec::new();
        panel.answered(id, CliOutcome::Done(String::new()), &cx(true), &mut again);
        assert!(again.is_empty(), "an answer counts once");
    }

    #[test]
    fn deleting_waits_for_the_letter_and_logs_go_to_the_run_tab() {
        let mut panel = DockerPanel {
            images: parse_images(r#"{"ID":"f00","Repository":"nginx","Tag":"1","Size":"1MB"}"#),
            section: DockerSection::Images,
            ..DockerPanel::default()
        };
        let mut out = Vec::new();
        panel.input(&Command::Insert("d".to_owned()), &cx(true), &mut out);
        assert!(out.is_empty() && panel.confirm.is_some());
        panel.confirm_input(&Command::Newline, &cx(true), &mut out);
        assert_eq!(out, [PanelReply::Info("Not run".to_owned())]);

        panel.containers = parse_containers(
            r#"{"ID":"abc","Names":"web-1","Image":"nginx","State":"running","Status":"Up"}"#,
        );
        panel.section = DockerSection::Containers;
        let mut out = Vec::new();
        panel.input(&Command::Insert("l".to_owned()), &cx(true), &mut out);
        let [PanelReply::Run(target)] = &out[..] else {
            panic!("{out:?}");
        };
        assert_eq!(target.name, "docker logs -f --tail 500 abc");
        assert_eq!(target.source, RunSource::Typed);
    }
}

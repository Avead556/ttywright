use super::Workspace;
use crate::core::action::Action;
use crate::core::command::{Command, Effect};
use crate::core::file_op::FilePromptKind;

fn runs_the_project(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::AnalyzeProject => "Analyze Project runs this project's code",
        Action::HttpRun => "Sending a request uses this project's addresses and env files",
        Action::ReformatCode => "Formatters load this project's config, which can be code",
        Action::DebugChoose | Action::DebugLast => "Debugging builds and runs this project",
        Action::ShowGitHub => "gh reads this repository's config, which can run programs",
        Action::RunTest => "Running a test runs this project's code",
        Action::SelectLanguageServer => "Listing servers asks this project's Rust toolchain",
        _ if action.spec().category == "Git" => "A repository's config can make git run programs",
        _ => return None,
    })
}

impl Workspace {
    pub fn is_trusted(&self) -> bool {
        self.trusted
    }

    pub fn configure_trust(&mut self, decided: Option<bool>) {
        self.trusted = decided == Some(true);
        if decided.is_none() && !self.window {
            self.trust_ask(None);
        }
        if !self.trusted && self.window {
            self.info("This project is not trusted: trust it in the main window".to_owned());
        }
    }

    pub fn without_untrusted(&self, mut effects: Vec<Effect>) -> Vec<Effect> {
        if !self.trusted {
            effects.retain(|e| !e.needs_trust());
        }
        effects
    }

    pub(super) fn trust_ask(&mut self, why: Option<String>) {
        if self.trusted {
            return self.info("This project is already trusted".to_owned());
        }
        if self.window {
            return self
                .info("This project is not trusted: trust it in the main window".to_owned());
        }
        self.file_prompt_open(FilePromptKind::TrustProject { why }, "");
    }

    pub(super) fn trust_refuses(&mut self, action: Action) -> bool {
        if self.trusted {
            return false;
        }
        let Some(why) = runs_the_project(action) else {
            return false;
        };
        self.trust_ask(Some(why.to_owned()));
        true
    }

    pub(super) fn trust_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let trusted = match input {
            Command::Escape => {
                self.file_prompt = None;
                return;
            }
            Command::Insert(s) if s.eq_ignore_ascii_case("y") => true,
            Command::Insert(s) if s.eq_ignore_ascii_case("n") => false,
            _ => return,
        };
        self.file_prompt = None;
        self.trusted = trusted;
        fx.push(Effect::SaveTrust {
            root: self.root.clone(),
            trusted,
        });
        if trusted {
            self.info("Trusted".to_owned());
            self.git_refresh_all(fx);
            let held = std::mem::take(&mut self.untrusted_swaps);
            if !held.is_empty() {
                self.recover_swaps(held);
            }
        } else {
            self.info("Not trusted: `Trust This Project` changes that".to_owned());
        }
    }
}

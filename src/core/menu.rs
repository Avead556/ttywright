use super::action::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry {
    Item(Action),
    Separator,
}

use MenuEntry::{Item, Separator};

pub struct Menu {
    pub title: &'static str,
    pub entries: &'static [MenuEntry],
}

pub const MENUS: &[Menu] = &[
    Menu {
        title: "File",
        entries: &[
            Item(Action::NewFile),
            Item(Action::NewFolder),
            Item(Action::NewFromTemplate),
            Item(Action::NewProject),
            Item(Action::NewScratchFile),
            Item(Action::ShowScratches),
            Item(Action::TrustProject),
            Separator,
            Item(Action::Save),
            Item(Action::SaveAll),
            Item(Action::ReloadFromDisk),
            Item(Action::ShowLocalHistory),
            Separator,
            Item(Action::RenameFile),
            Item(Action::DeleteFile),
            Separator,
            Item(Action::CloseTab),
            Item(Action::OpenSettings),
            Item(Action::Quit),
        ],
    },
    Menu {
        title: "Edit",
        entries: &[
            Item(Action::MacroRecord),
            Item(Action::MacroPlay),
            Separator,
            Item(Action::Undo),
            Item(Action::Redo),
            Separator,
            Item(Action::Cut),
            Item(Action::Copy),
            Item(Action::Paste),
            Item(Action::SelectAll),
            Separator,
            Item(Action::Find),
            Item(Action::FindNext),
            Item(Action::FindPrevious),
            Item(Action::Replace),
            Item(Action::FindInProject),
            Item(Action::ReplaceInFiles),
        ],
    },
    Menu {
        title: "View",
        entries: &[
            Item(Action::ToggleProject),
            Item(Action::ShowProblems),
            Item(Action::ShowLanguageServer),
            Item(Action::ShowHttp),
            Item(Action::ShowDatabase),
            Item(Action::ShowDependencies),
            Item(Action::ShowNotifications),
            Item(Action::ClearNotifications),
            Item(Action::ShowDocker),
            Item(Action::ShowGitHub),
            Item(Action::ShowRun),
            Item(Action::TerminalToggle),
            Item(Action::TerminalNew),
            Item(Action::HideToolWindow),
            Separator,
            Item(Action::ToggleSoftWrap),
            Item(Action::MarkdownPreview),
            Item(Action::OpenInSystemViewer),
            Separator,
            Item(Action::SplitVertically),
            Item(Action::SplitHorizontally),
            Item(Action::Unsplit),
            Item(Action::NextSplitter),
            Separator,
            Item(Action::NextTab),
            Item(Action::PreviousTab),
        ],
    },
    Menu {
        title: "Navigate",
        entries: &[
            Item(Action::SearchEverywhere),
            Item(Action::GotoFile),
            Item(Action::RecentFiles),
            Separator,
            Item(Action::GotoDefinition),
            Item(Action::NavigateBack),
            Separator,
            Item(Action::GotoSymbol),
            Item(Action::FileStructure),
            Item(Action::CallersOf),
            Item(Action::CalleesOf),
            Separator,
            Item(Action::ToggleBookmark),
            Item(Action::ShowBookmarks),
            Item(Action::ShowTodos),
        ],
    },
    Menu {
        title: "Code",
        entries: &[
            Item(Action::Completion),
            Item(Action::QuickDocumentation),
            Item(Action::FindUsages),
            Item(Action::RenameSymbol),
            Item(Action::RefactorThis),
            Item(Action::SafeDelete),
            Item(Action::StructuralSearch),
            Item(Action::LocateDuplicates),
            Item(Action::ContextActions),
            Item(Action::ReformatCode),
            Item(Action::CheckSpelling),
            Item(Action::AddToDictionary),
            Separator,
            Item(Action::AnalyzeProject),
            Item(Action::StopAnalysis),
            Separator,
            Item(Action::SelectLanguageServer),
            Item(Action::RestartLanguageServer),
        ],
    },
    Menu {
        title: "Git",
        entries: &[
            Item(Action::GitCommit),
            Item(Action::GitLog),
            Item(Action::GitPush),
            Item(Action::GitPull),
            Separator,
            Item(Action::GitBranches),
            Item(Action::GitManageBranches),
            Item(Action::GitStash),
            Separator,
            Item(Action::GitFileHistory),
            Item(Action::GitCompareBranch),
            Item(Action::GitComparePath),
            Item(Action::GitBlame),
            Separator,
            Item(Action::GitMergeView),
            Item(Action::GitContinue),
            Item(Action::GitAbort),
            Separator,
            Item(Action::GitLogWindow),
        ],
    },
    Menu {
        title: "Run",
        entries: &[
            Item(Action::RunChoose),
            Item(Action::RunAnything),
            Item(Action::RunTest),
            Item(Action::ShowTests),
            Item(Action::RerunLast),
            Item(Action::StopRun),
            Separator,
            Item(Action::NextRunError),
            Item(Action::PreviousRunError),
            Item(Action::RunOutputWindow),
            Separator,
            Item(Action::HttpRun),
            Item(Action::HttpStop),
            Item(Action::DbRun),
            Separator,
            Item(Action::LoadCoverage),
            Item(Action::ToggleCoverage),
            Item(Action::DbStop),
        ],
    },
    Menu {
        title: "Debug",
        entries: &[
            Item(Action::DebugChoose),
            Item(Action::DebugLast),
            Item(Action::DebugStop),
            Item(Action::DebugShow),
            Separator,
            Item(Action::ResumeProgram),
            Item(Action::DebugPause),
            Item(Action::DebugStepOver),
            Item(Action::DebugStepInto),
            Item(Action::DebugStepOut),
            Item(Action::DebugRunToCursor),
            Separator,
            Item(Action::ToggleBreakpoint),
            Item(Action::EditBreakpoint),
            Item(Action::AddLogpoint),
            Item(Action::ToggleBreakpointEnabled),
            Item(Action::ViewBreakpoints),
            Item(Action::RemoveAllBreakpoints),
            Item(Action::DebugExceptions),
            Separator,
            Item(Action::DebugEvaluate),
            Item(Action::DebugAddWatch),
        ],
    },
    Menu {
        title: "Claude",
        entries: &[
            Item(Action::ClaudeOpen),
            Item(Action::ClaudeAsk),
            Item(Action::ClaudeFix),
            Separator,
            Item(Action::ClaudeReview),
            Separator,
            Item(Action::ClaudeWindow),
        ],
    },
    Menu {
        title: "Help",
        entries: &[Item(Action::OpenPalette)],
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    pub menu: usize,
    pub entry: usize,
}

impl MenuState {
    pub fn open(menu: usize) -> Self {
        let menu = menu.min(MENUS.len() - 1);
        Self { menu, entry: 0 }
    }

    pub fn cycle(&mut self, forward: bool) {
        let count = MENUS.len();
        *self = Self::open(if forward {
            (self.menu + 1) % count
        } else {
            (self.menu + count - 1) % count
        });
    }

    pub fn step(&mut self, forward: bool) {
        let entries = MENUS[self.menu].entries;
        let count = entries.len();
        for n in 1..count {
            let at = if forward {
                (self.entry + n) % count
            } else {
                (self.entry + count - n) % count
            };
            if entries[at] != Separator {
                self.entry = at;
                return;
            }
        }
    }

    pub fn action(&self, entry: usize) -> Option<Action> {
        match MENUS.get(self.menu)?.entries.get(entry)? {
            Item(action) => Some(*action),
            Separator => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_an_action_of_the_palette_too() {
        for menu in MENUS {
            assert_ne!(menu.entries.first(), Some(&Separator), "{}", menu.title);
            for entry in menu.entries {
                if let Item(action) = entry {
                    assert!(
                        Action::ALL.contains(action) || *action == Action::Quit,
                        "{action:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn steps_skip_separators_and_go_round() {
        let mut state = MenuState::open(0);
        state.step(true);
        state.step(true);
        assert_eq!(state.action(state.entry), Some(Action::NewFromTemplate));
        state.step(false);
        assert_eq!(state.action(state.entry), Some(Action::NewFolder));
        state.step(false);
        state.step(false);
        assert_eq!(state.action(state.entry), Some(Action::Quit));
    }

    #[test]
    fn switching_menus_starts_at_the_top_and_goes_round() {
        let mut state = MenuState { menu: 0, entry: 3 };
        state.cycle(false);
        assert_eq!((state.menu, state.entry), (MENUS.len() - 1, 0));
        state.cycle(true);
        assert_eq!((state.menu, state.entry), (0, 0));
    }
}

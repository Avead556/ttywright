use std::path::{Path, PathBuf};

use super::{Focus, Workspace, move_input};
use crate::core::action::Action;
use crate::core::command::{Command, Effect, Motion};
use crate::core::file_op::{FileOp, FilePrompt, FilePromptKind, name_under};
use crate::core::input::LineInput;
use crate::core::menu::MenuState;
use crate::core::picker::PickerKind;
use crate::core::templates::{
    FILE_TEMPLATES, PROJECT_TEMPLATES, file_name, initial_input, render_file, render_project,
};

impl Workspace {
    pub fn menu(&self) -> Option<MenuState> {
        self.menu
    }

    pub fn file_prompt(&self) -> Option<&FilePrompt> {
        self.file_prompt.as_ref()
    }

    pub(super) fn menu_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(state) = self.menu.as_mut() else {
            return;
        };
        match input {
            Command::Escape | Command::Run(Action::MainMenu) => self.menu = None,
            Command::Move(Motion::Left) => state.cycle(false),
            Command::Move(Motion::Right) => state.cycle(true),
            Command::Move(Motion::Up) => state.step(false),
            Command::Move(Motion::Down) => state.step(true),
            Command::OpenMenu { menu } if state.menu == menu => self.menu = None,
            Command::OpenMenu { menu } => *state = MenuState::open(menu),
            Command::Newline => {
                let entry = state.entry;
                self.menu_run(entry, fx);
            }
            Command::Insert(s) if s == " " => {
                let entry = state.entry;
                self.menu_run(entry, fx);
            }
            Command::PickMenuItem { entry } => self.menu_run(entry, fx),
            _ => {}
        }
    }

    fn menu_run(&mut self, entry: usize, fx: &mut Vec<Effect>) {
        let action = self.menu.and_then(|state| state.action(entry));
        self.menu = None;
        if let Some(action) = action {
            self.run_action(action, fx);
        }
    }

    pub(super) fn template_chosen(&mut self, kind: PickerKind, index: usize) {
        match kind {
            PickerKind::FileTemplates => {
                let dir = self.new_file_dir();
                let text = FILE_TEMPLATES.get(index).map_or("", initial_input);
                self.file_prompt_open(FilePromptKind::FromTemplate { dir, index }, text);
            }
            _ => {
                let parent = self.root.parent().unwrap_or(&self.root);
                let suggested = parent.join("my-project").display().to_string();
                self.file_prompt_open(FilePromptKind::NewProject { index }, &suggested);
            }
        }
    }

    fn project_op(&self, index: usize, typed: &str) -> Result<FileOp, String> {
        let typed = typed.trim();
        if typed.is_empty() {
            return Err("Type where to create it".to_owned());
        }
        let parent = self.root.parent().unwrap_or(&self.root);
        let dir = parent.join(typed);
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        let (Some(template), Some(folder)) = (PROJECT_TEMPLATES.get(index), folder) else {
            return Err("That is not a folder for a project".to_owned());
        };
        Ok(FileOp::CreateProject {
            files: render_project(template, &folder),
            dir,
        })
    }

    pub(super) fn file_target(&self) -> Option<PathBuf> {
        let path = if self.focus == Focus::Project {
            self.tree.selected().to_path_buf()
        } else {
            self.active()?.1.path().to_path_buf()
        };
        (path != self.root).then_some(path)
    }

    fn new_file_dir(&self) -> PathBuf {
        let Some(path) = self.file_target() else {
            return self.root.clone();
        };
        let is_dir = self.focus == Focus::Project
            && self.tree.rows().iter().any(|r| r.path == path && r.is_dir);
        if is_dir {
            return path;
        }
        path.parent()
            .map_or_else(|| self.root.clone(), Path::to_path_buf)
    }

    pub(super) fn file_prompt_open(&mut self, kind: FilePromptKind, text: &str) {
        let mut input = LineInput::default();
        input.insert(text);
        self.file_prompt = Some(FilePrompt {
            kind,
            input,
            error: None,
            busy: false,
        });
    }

    pub(super) fn file_action(&mut self, action: Action) {
        if self.window {
            return self.info("Files are changed from the main window".to_owned());
        }
        match action {
            Action::NewFile => {
                let dir = self.new_file_dir();
                self.file_prompt_open(FilePromptKind::NewFile { dir }, "");
            }
            Action::NewFolder => {
                let dir = self.new_file_dir();
                self.file_prompt_open(FilePromptKind::NewFolder { dir }, "");
            }
            Action::RenameFile | Action::DeleteFile => {
                let Some(path) = self.file_target() else {
                    return self.info("Choose a file in the Project tree first".to_owned());
                };
                if self.documents_under(&path).iter().any(|(_, dirty)| *dirty) {
                    return self.error("It has unsaved changes: save them first".to_owned());
                }
                if action == Action::RenameFile {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    let name = name.unwrap_or_default();
                    self.file_prompt_open(FilePromptKind::Rename { path }, &name);
                } else {
                    self.file_prompt_open(FilePromptKind::Delete { path }, "");
                }
            }
            _ => {}
        }
    }

    fn documents_under(&self, path: &Path) -> Vec<(PathBuf, bool)> {
        self.documents
            .values()
            .filter(|d| d.path().starts_with(path))
            .map(|d| (d.path().to_path_buf(), d.is_dirty()))
            .collect()
    }

    pub(super) fn file_prompt_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(prompt) = self.file_prompt.as_mut() else {
            return;
        };
        let asks_yes_no = matches!(
            prompt.kind,
            FilePromptKind::Delete { .. }
                | FilePromptKind::RenameConfirm { .. }
                | FilePromptKind::SafeDeleteConfirm { .. }
                | FilePromptKind::TrustProject { .. }
        );
        if matches!(prompt.kind, FilePromptKind::TrustProject { .. }) {
            return self.trust_input(&input, fx);
        }
        match input {
            Command::Escape => self.file_prompt_dismiss(),
            _ if prompt.busy => {}
            Command::Newline if matches!(prompt.kind, FilePromptKind::SafeDeleteConfirm { .. }) => {
            }
            Command::Newline => self.file_prompt_confirm(fx),
            Command::Insert(s) if asks_yes_no => match s.to_lowercase().as_str() {
                "y" => self.file_prompt_confirm(fx),
                "n" => self.file_prompt_dismiss(),
                _ => {}
            },
            Command::Insert(s) => {
                prompt.input.insert(&s);
                prompt.error = None;
            }
            Command::DeleteBackward => {
                prompt.input.delete_backward();
                prompt.error = None;
            }
            Command::DeleteForward => {
                prompt.input.delete_forward();
                prompt.error = None;
            }
            Command::Move(Motion::Up) if is_run_anything(&prompt.kind) => {
                self.run_anything_history_step(true);
            }
            Command::Move(Motion::Down) if is_run_anything(&prompt.kind) => {
                self.run_anything_history_step(false);
            }
            Command::Move(motion) => move_input(&mut prompt.input, motion),
            _ => {}
        }
    }

    fn file_prompt_dismiss(&mut self) {
        if let Some(FilePromptKind::SafeDeleteConfirm { .. }) =
            self.file_prompt.as_ref().map(|p| &p.kind)
        {
            self.safe_delete_declined();
        }
        if let Some(FilePromptKind::RenameConfirm { .. }) =
            self.file_prompt.as_ref().map(|p| &p.kind)
        {
            self.rename_declined();
        }
        self.file_prompt = None;
    }

    fn file_prompt_confirm(&mut self, fx: &mut Vec<Effect>) {
        let Some(prompt) = self.file_prompt.as_ref() else {
            return;
        };
        if let FilePromptKind::RenameSymbol { old } = &prompt.kind {
            let (old, name) = (old.clone(), prompt.input.text().trim().to_owned());
            return self.rename_named(&old, name, fx);
        }
        if is_run_anything(&prompt.kind) {
            let text = prompt.input.text().trim().to_owned();
            return self.run_anything_confirmed(text, fx);
        }
        if matches!(prompt.kind, FilePromptKind::CoverageReport) {
            let typed = prompt.input.text().to_owned();
            self.file_prompt = None;
            return self.coverage_named(&typed, fx);
        }
        if matches!(prompt.kind, FilePromptKind::SafeDeleteConfirm { .. }) {
            self.file_prompt = None;
            return self.safe_delete_confirmed();
        }
        if matches!(prompt.kind, FilePromptKind::RenameConfirm { .. }) {
            self.file_prompt = None;
            return self.rename_confirmed(fx);
        }
        if matches!(
            prompt.kind,
            FilePromptKind::BreakpointCondition { .. }
                | FilePromptKind::BreakpointLog { .. }
                | FilePromptKind::DebugEvaluate
                | FilePromptKind::DebugWatch
        ) {
            let kind = prompt.kind.clone();
            let text = prompt.input.text().to_owned();
            self.file_prompt = None;
            return self.debug_prompt_confirm(kind, text, fx);
        }
        let name = prompt.input.text();
        let planned = match &prompt.kind {
            FilePromptKind::NewFile { dir } => name_under(dir, name).map(FileOp::NewFile),
            FilePromptKind::FromTemplate { dir, index } => from_template(dir, *index, name),
            FilePromptKind::NewProject { index } => self.project_op(*index, name),
            FilePromptKind::NewFolder { dir } => name_under(dir, name).map(FileOp::NewFolder),
            FilePromptKind::Rename { path } => {
                let dir = path.parent().unwrap_or(&self.root);
                name_under(dir, name).map(|to| FileOp::Rename {
                    from: path.clone(),
                    to,
                })
            }
            FilePromptKind::Delete { path } => Ok(FileOp::Trash(path.clone())),
            FilePromptKind::BreakpointCondition { .. }
            | FilePromptKind::BreakpointLog { .. }
            | FilePromptKind::DebugEvaluate
            | FilePromptKind::DebugWatch
            | FilePromptKind::RenameSymbol { .. }
            | FilePromptKind::RenameConfirm { .. }
            | FilePromptKind::SafeDeleteConfirm { .. }
            | FilePromptKind::CoverageReport
            | FilePromptKind::TrustProject { .. }
            | FilePromptKind::RunAnything { .. } => return,
        };
        let op = planned.and_then(|op| match &op {
            FileOp::Rename { from, to } if from == to => Err("That is its name already".to_owned()),
            _ => Ok(op),
        });
        let op = match op {
            Ok(op) => op,
            Err(message) => {
                if let Some(prompt) = self.file_prompt.as_mut() {
                    prompt.error = Some(message);
                }
                return;
            }
        };
        if let FileOp::Rename { from: path, .. } | FileOp::Trash(path) = &op
            && !self.file_op_close_tabs(&path.clone(), fx)
        {
            if let Some(prompt) = self.file_prompt.as_mut() {
                prompt.error = Some("It has unsaved changes: save them first".to_owned());
            }
            return;
        }
        if let Some(prompt) = self.file_prompt.as_mut() {
            prompt.busy = true;
            prompt.error = None;
        }
        fx.push(Effect::FileOp(op));
    }

    fn file_op_close_tabs(&mut self, path: &Path, fx: &mut Vec<Effect>) -> bool {
        let under = self.documents_under(path);
        if under.iter().any(|(_, dirty)| *dirty) {
            return false;
        }
        let views: Vec<_> = self
            .views
            .iter()
            .filter(|(_, v)| {
                self.documents
                    .get(&v.doc)
                    .is_some_and(|d| d.path().starts_with(path))
            })
            .map(|(id, _)| *id)
            .collect();
        for view in views {
            self.close_view(view, fx);
        }
        self.file_op_reopen = under.into_iter().map(|(p, _)| p).collect();
        true
    }

    pub(super) fn file_op_done(
        &mut self,
        op: &FileOp,
        result: Result<(), String>,
        fx: &mut Vec<Effect>,
    ) {
        let reopen = std::mem::take(&mut self.file_op_reopen);
        if let Err(message) = result {
            for path in reopen {
                self.open_file(path, None, fx);
            }
            match self.file_prompt.as_mut() {
                Some(prompt) => {
                    prompt.busy = false;
                    prompt.error = Some(message);
                }
                None => self.error(message),
            }
            return;
        }
        self.file_prompt = None;
        let mut changed = Vec::new();
        match op {
            FileOp::NewFile(path) => {
                self.open_file(path.clone(), None, fx);
                self.focus = Focus::Editor;
                changed.push(path.clone());
            }
            FileOp::NewFolder(path) => changed.push(path.clone()),
            FileOp::Create { path, .. } => {
                self.open_file(path.clone(), None, fx);
                self.focus = Focus::Editor;
                changed.push(path.clone());
            }
            FileOp::CreateProject { dir, .. } => {
                self.info(format!("Created {}", dir.display()));
                fx.push(Effect::SaveTrust {
                    root: dir.clone(),
                    trusted: true,
                });
                fx.push(Effect::OpenProject { dir: dir.clone() });
                return;
            }
            FileOp::Rename { from, to } => {
                for path in reopen {
                    if let Ok(inner) = path.strip_prefix(from) {
                        let moved = if inner.as_os_str().is_empty() {
                            to.clone()
                        } else {
                            to.join(inner)
                        };
                        self.open_file(moved, None, fx);
                    }
                }
                changed.push(from.clone());
                changed.push(to.clone());
            }
            FileOp::Trash(path) => changed.push(path.clone()),
        }
        let root = self.root.clone();
        let ancestors: Vec<PathBuf> = changed
            .iter()
            .flat_map(|p| p.ancestors().skip(1))
            .filter(|p| p.starts_with(&root) && *p != root)
            .map(Path::to_path_buf)
            .collect();
        changed.extend(ancestors);
        self.paths_changed(&changed, fx);
    }
}

const fn is_run_anything(kind: &FilePromptKind) -> bool {
    matches!(kind, FilePromptKind::RunAnything { .. })
}

fn from_template(dir: &Path, index: usize, typed: &str) -> Result<FileOp, String> {
    let template = FILE_TEMPLATES.get(index).ok_or("No such template")?;
    let name = file_name(template, typed)?;
    let path = name_under(dir, &name)?;
    let folder = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(FileOp::Create {
        contents: render_file(template, &name, &folder),
        executable: template.executable,
        path,
    })
}

use super::{DocumentId, Focus, QuitState, Workspace};
use crate::core::command::Effect;
use crate::core::plugin::{
    PluginContext, PluginInfo, PluginReport, acts_at_the_cursor, clean_text,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PluginRun {
    plugin: String,
    place: Option<(DocumentId, u64, usize)>,
}

impl Workspace {
    fn plugin_commands(&self) -> impl Iterator<Item = (&PluginInfo, usize)> {
        self.plugins
            .iter()
            .flat_map(|p| (0..p.commands.len()).map(move |i| (p, i)))
    }

    pub(super) fn plugin_titles(&self) -> impl Iterator<Item = String> + '_ {
        self.plugin_commands()
            .map(|(p, i)| format!("Plugin: {}", p.commands[i].title))
    }

    pub(super) fn plugin_chosen(&mut self, index: usize, fx: &mut Vec<Effect>) {
        let Some((plugin, i)) = self.plugin_commands().nth(index) else {
            return;
        };
        let (plugin, command) = (plugin.name.clone(), plugin.commands[i].id.clone());
        if let Some(busy) = &self.plugin_running {
            let message = format!("{} is still working", busy.plugin);
            return self.info(message);
        }
        let active = self.active().map(|(view, d)| {
            let selection = view.cursor.selection().map(|r| (r.start, r.end));
            let context = PluginContext {
                path: Some(d.path().to_path_buf()),
                text: Some(d.text().to_string()),
                cursor: Some(view.cursor.idx),
                selection,
            };
            (context, (view.doc, d.version(), view.cursor.idx))
        });
        let (context, place) =
            active.map_or((PluginContext::default(), None), |(c, p)| (c, Some(p)));
        self.plugin_running = Some(PluginRun {
            plugin: plugin.clone(),
            place,
        });
        fx.push(Effect::RunPlugin {
            plugin,
            command,
            root: self.root.clone(),
            context,
        });
    }

    pub(super) fn plugins_loaded(&mut self, plugins: Vec<PluginInfo>, problems: Vec<String>) {
        self.plugins = plugins;
        if let Some(first) = problems.first() {
            let more = problems.len() - 1;
            let tail = if more > 0 {
                format!(" (+{more} more)")
            } else {
                String::new()
            };
            self.error(format!("{}{tail}", clean_text(first)));
        }
    }

    pub(super) fn plugin_done(
        &mut self,
        plugin: &str,
        result: Result<PluginReport, String>,
    ) -> Vec<Effect> {
        let running = self.plugin_running.take_if(|r| r.plugin == plugin);
        let name = clean_text(plugin);
        let report = match result {
            Ok(report) => report,
            Err(message) => {
                self.error(format!("{name}: {}", clean_text(&message)));
                return Vec::new();
            }
        };
        let mut fx = Vec::new();
        if report.commands.iter().any(acts_at_the_cursor) {
            let current = self
                .active()
                .map(|(view, d)| (view.doc, d.version(), view.cursor.idx));
            let undisturbed = self.focus == Focus::Editor
                && self.picker.is_none()
                && self.file_prompt.is_none()
                && current.is_some()
                && running.is_some_and(|r| r.place == current);
            if !undisturbed {
                self.error(format!(
                    "{name}: the text changed while it worked; nothing applied"
                ));
                return fx;
            }
        }
        for command in report.commands {
            fx.extend(self.dispatch(command));
            if self.quit != QuitState::Running {
                break;
            }
        }
        if let Some(status) = report.status {
            self.info(format!("{name}: {}", clean_text(&status)));
        }
        fx
    }
}

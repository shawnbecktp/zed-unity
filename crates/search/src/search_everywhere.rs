use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use command_palette::{available_commands, normalize_action_query};
use editor::{Editor, SelectionEffects, scroll::Autoscroll};
use fuzzy_nucleo::{MaxResults, StringMatchCandidate, StringMatchOptions};
use gpui::{
    Action, AsyncApp, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription,
    Task, WeakEntity, actions,
};
use language::Buffer;
use picker::{Picker, PickerDelegate};
use project::{
    Candidates, PathMatchCandidateSet, Project, ProjectPath, Symbol, WorktreeId,
    lsp_store::SymbolLocation,
    search::{SearchQuery, SearchResult},
};
use text::{Anchor, Bias};
use ui::{KeyBinding, ListItem, ListItemSpacing, prelude::*};
use util::{ResultExt, rel_path::RelPath};
use workspace::{ModalView, Workspace, notifications::NotifyTaskExt};

use crate::text_finder::{SearchMatch, render_matched_line};

actions!(
    search,
    [
        /// Searches for project files, symbols, editor actions, and text.
        ToggleSearchEverywhere,
        /// Selects the next Search Everywhere category.
        NextSearchCategory,
        /// Selects the previous Search Everywhere category.
        PreviousSearchCategory
    ]
);

const DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Category {
    All,
    Files,
    Symbols,
    Actions,
    Text,
}

impl Category {
    const TABS: [Self; 5] = [
        Self::All,
        Self::Files,
        Self::Symbols,
        Self::Actions,
        Self::Text,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Files => "Files",
            Self::Symbols => "Symbols",
            Self::Actions => "Actions",
            Self::Text => "Text",
        }
    }
    fn includes(self, category: Self) -> bool {
        self == Self::All || self == category
    }
    fn limit(self) -> NonZeroUsize {
        if self == Self::All {
            const { NonZeroUsize::new(5).unwrap() }
        } else {
            const { NonZeroUsize::new(100).unwrap() }
        }
    }
}

enum Hit {
    File(ProjectPath),
    Symbol(Symbol),
    Action(usize),
    Text(SearchMatch),
}

struct SearchEverywhere {
    picker: Entity<Picker<Delegate>>,
    _dismiss: Subscription,
}

pub(super) fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _: &mut Context<Workspace>| {
        workspace.register_action(|workspace, _: &ToggleSearchEverywhere, window, cx| {
            if workspace.active_modal::<SearchEverywhere>(cx).is_some() {
                workspace.hide_modal(window, cx);
                return;
            }
            if workspace.has_active_modal(window, cx) && !workspace.hide_modal(window, cx) {
                return;
            }
            let Some(origin) = window.focused(cx) else {
                return;
            };
            let mut commands: Vec<_> = available_commands(window, cx).collect();
            commands.sort_by(|a, b| a.0.cmp(&b.0));
            let recent = workspace
                .recent_navigation_history(Some(100), cx)
                .into_iter()
                .map(|(path, _)| path)
                .collect();
            let active = workspace
                .active_item(cx)
                .and_then(|item| item.project_path(cx));
            let delegate = Delegate {
                workspace: cx.weak_entity(),
                project: workspace.project().clone(),
                origin,
                commands,
                recent,
                active,
                category: Category::All,
                results: BTreeMap::new(),
                selected: None,
                generation: 0,
                cancel: Default::default(),
                searches: Vec::new(),
                error: None,
            };
            workspace.toggle_modal(window, cx, move |window, cx| {
                let picker = cx.new(|cx| {
                    Picker::uniform_list(delegate, window, cx)
                        .initial_width(rems(44.))
                        .show_scrollbar(true)
                        .reopenable(false, cx)
                });
                let dismiss =
                    cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent));
                SearchEverywhere {
                    picker,
                    _dismiss: dismiss,
                }
            });
        });
    })
    .detach();
}

impl ModalView for SearchEverywhere {}
impl EventEmitter<DismissEvent> for SearchEverywhere {}
impl Focusable for SearchEverywhere {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}
impl SearchEverywhere {
    fn cycle(&self, forward: bool, window: &mut Window, cx: &mut App) {
        self.picker.update(cx, |picker, cx| {
            let tabs = Category::TABS;
            let current = tabs
                .iter()
                .position(|tab| *tab == picker.delegate.category)
                .unwrap_or_default();
            let step = if forward { 1 } else { tabs.len() - 1 };
            select_category(picker, tabs[(current + step) % tabs.len()], window, cx);
        });
    }
}
impl Render for SearchEverywhere {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("SearchEverywhere")
            .on_action(
                cx.listener(|this, _: &NextSearchCategory, window, cx| {
                    this.cycle(true, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &PreviousSearchCategory, window, cx| {
                this.cycle(false, window, cx)
            }))
            .child(self.picker.clone())
    }
}

fn select_category(
    picker: &mut Picker<Delegate>,
    category: Category,
    window: &mut Window,
    cx: &mut Context<Picker<Delegate>>,
) {
    if picker.delegate.category != category {
        picker.delegate.category = category;
        picker.refresh(window, cx);
    }
    picker.focus(window, cx);
}

struct Delegate {
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    origin: FocusHandle,
    commands: Vec<(SharedString, Box<dyn Action>)>,
    recent: Vec<ProjectPath>,
    active: Option<ProjectPath>,
    category: Category,
    results: BTreeMap<Category, Vec<Hit>>,
    selected: Option<(Category, usize)>,
    generation: usize,
    cancel: Arc<AtomicBool>,
    searches: Vec<Task<()>>,
    error: Option<SharedString>,
}

impl Drop for Delegate {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Delegate {
    fn rows(&self) -> impl Iterator<Item = (Category, Option<usize>)> + '_ {
        self.results
            .iter()
            .filter(|(_, hits)| !hits.is_empty())
            .flat_map(|(&category, hits)| {
                let header = (self.category == Category::All).then_some((category, None));
                header
                    .into_iter()
                    .chain((0..hits.len()).map(move |index| (category, Some(index))))
            })
    }
    fn path_label(&self, path: &ProjectPath, cx: &App) -> String {
        let project = self.project.read(cx);
        if path.path.is_empty()
            && let Some(tree) = project.worktree_for_id(path.worktree_id, cx)
        {
            return tree.read(cx).root_name().as_unix_str().to_owned();
        }
        project
            .short_full_path_for_project_path(path, cx)
            .unwrap_or_else(|| path.path.as_unix_str().to_owned())
    }
    fn publish(
        picker: &WeakEntity<Picker<Self>>,
        generation: usize,
        category: Category,
        hits: Vec<Hit>,
        cx: &mut AsyncApp,
    ) {
        if hits.is_empty() {
            return;
        }
        if let Some(picker) = picker.upgrade() {
            picker.update(cx, |picker, cx| {
                if picker.delegate.generation == generation {
                    picker
                        .delegate
                        .results
                        .entry(category)
                        .or_default()
                        .extend(hits);
                    cx.notify();
                }
            });
        }
    }
    fn files(&self, query: String, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let project = self.project.clone();
        let trees = project
            .read(cx)
            .worktree_store()
            .read(cx)
            .visible_worktrees_and_single_files(cx)
            .collect::<Vec<_>>();
        let mut single_files = collections::HashSet::default();
        let multiple = trees.len() > 1;
        let sets = trees
            .iter()
            .map(|tree| {
                let tree = tree.read(cx);
                if tree.is_single_file() {
                    single_files.insert(tree.id());
                }
                PathMatchCandidateSet {
                    snapshot: tree.snapshot(),
                    include_ignored: tree.root_entry().is_some_and(|root| root.is_ignored),
                    include_root_name: multiple || tree.is_single_file(),
                    candidates: Candidates::Files,
                }
            })
            .collect::<Vec<_>>();
        let relative = self.active.as_ref().map(|path| path.path.clone());
        let max_results = MaxResults::Limit(self.category.limit());
        let cancel = self.cancel.clone();
        let generation = self.generation;
        cx.spawn(async move |picker, cx| {
            let matches = fuzzy_nucleo::match_path_sets(
                &sets,
                &query,
                &relative,
                fuzzy_nucleo::PathMatchOptions {
                    max_results,
                    cancel_flag: Some(&cancel),
                    ..Default::default()
                },
                cx.background_executor().clone(),
            )
            .await;
            let hits = matches
                .into_iter()
                .map(|item| {
                    let worktree_id = WorktreeId::from_usize(item.worktree_id);
                    let path = if single_files.contains(&worktree_id) {
                        RelPath::empty_arc()
                    } else {
                        item.path
                    };
                    Hit::File(ProjectPath { worktree_id, path })
                })
                .collect();
            Self::publish(&picker, generation, Category::Files, hits, cx);
        })
    }
    fn actions(&self, query: &str, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let candidates = self
            .commands
            .iter()
            .enumerate()
            .map(|(id, (name, _))| StringMatchCandidate::from_shared(id, name.clone()))
            .collect::<Vec<_>>();
        let query = normalize_action_query(query);
        let max_results = MaxResults::Limit(self.category.limit());
        let cancel = self.cancel.clone();
        let generation = self.generation;
        cx.spawn(async move |picker, cx| {
            let matches = fuzzy_nucleo::match_strings_async(
                &candidates,
                &query,
                StringMatchOptions {
                    max_results,
                    cancel_flag: Some(&cancel),
                    ..Default::default()
                },
                cx.background_executor().clone(),
            )
            .await;
            let hits = matches
                .into_iter()
                .map(|item| Hit::Action(item.candidate_id))
                .collect();
            Self::publish(&picker, generation, Category::Actions, hits, cx);
        })
    }
    fn symbols(&self, query: String, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let project = self.project.clone();
        let max_results = MaxResults::Limit(self.category.limit());
        let cancel = self.cancel.clone();
        let generation = self.generation;
        cx.spawn(async move |picker, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let result = project
                .update(cx, |project, cx| project.symbols(&query, cx))
                .await;
            let Some(symbols) = result.log_err() else {
                if let Some(picker) = picker.upgrade() {
                    picker.update(cx, |picker, cx| {
                        if picker.delegate.generation == generation {
                            picker.delegate.error = Some("Symbol search unavailable".into());
                            cx.notify();
                        }
                    });
                }
                return;
            };
            let candidates = symbols
                .iter()
                .enumerate()
                .map(|(id, symbol)| StringMatchCandidate::new(id, symbol.label.filter_text()))
                .collect::<Vec<_>>();
            let filter = query.rsplit("::").next().unwrap_or(&query);
            let matches = fuzzy_nucleo::match_strings_async(
                &candidates,
                filter,
                StringMatchOptions {
                    max_results,
                    cancel_flag: Some(&cancel),
                    ..Default::default()
                },
                cx.background_executor().clone(),
            )
            .await;
            let hits = matches
                .into_iter()
                .filter_map(|item| symbols.get(item.candidate_id).cloned().map(Hit::Symbol))
                .collect();
            Self::publish(&picker, generation, Category::Symbols, hits, cx);
        })
    }
    fn text(&self, query: String, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let project = self.project.clone();
        let generation = self.generation;
        let mut remaining = self.category.limit().get();
        cx.spawn(async move |picker, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let Some(query) = SearchQuery::text(
                query,
                false,
                false,
                false,
                Default::default(),
                Default::default(),
                false,
                None,
            )
            .log_err() else {
                return;
            };
            let search = project.update(cx, |project, cx| project.search(query, cx));
            while let Ok(result) = search.rx.recv().await {
                match result {
                    SearchResult::Buffer { buffer, ranges } => {
                        let count = remaining.min(ranges.len());
                        let hits = SearchMatch::from_ranges(&buffer, &ranges[..count], cx)
                            .into_iter()
                            .map(Hit::Text)
                            .collect::<Vec<_>>();
                        remaining -= hits.len();
                        Self::publish(&picker, generation, Category::Text, hits, cx);
                        if remaining == 0 {
                            break;
                        }
                    }
                    SearchResult::LimitReached => break,
                    _ => {}
                }
            }
        })
    }
}

impl PickerDelegate for Delegate {
    type ListItem = ListItem;
    fn name() -> &'static str {
        "Search Everywhere"
    }
    fn match_count(&self) -> usize {
        self.rows().count()
    }
    fn selected_index(&self) -> usize {
        self.rows()
            .position(|(category, index)| {
                index.is_some_and(|index| {
                    self.selected
                        .is_none_or(|selected| selected == (category, index))
                })
            })
            .unwrap_or_default()
    }
    fn can_select(&self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) -> bool {
        self.rows()
            .nth(ix)
            .is_some_and(|(_, index)| index.is_some())
    }
    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let row = self.rows().nth(ix);
        if let Some((category, Some(index))) = row {
            self.selected = Some((category, index));
        }
    }
    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Search Everywhere…".into()
    }
    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        if self.searches.is_empty() {
            return Some(
                if self.category.includes(Category::Files) {
                    "No recent files"
                } else {
                    "Type to search"
                }
                .into(),
            );
        }
        Some(self.error.clone().unwrap_or_else(|| "No matches".into()))
    }
    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.generation += 1;
        self.cancel.store(true, Ordering::Release);
        self.cancel = Default::default();
        self.searches.clear();
        self.results.clear();
        self.selected = None;
        self.error = None;
        if query.is_empty() {
            if self.category.includes(Category::Files) {
                let mut seen = collections::HashSet::default();
                let project = self.project.read(cx);
                let hits = self
                    .recent
                    .iter()
                    .chain(self.active.iter())
                    .filter(|path| {
                        seen.insert((*path).clone())
                            && project
                                .entry_for_path(path, cx)
                                .is_some_and(|entry| entry.is_file())
                    })
                    .take(self.category.limit().get())
                    .cloned()
                    .map(Hit::File)
                    .collect();
                self.results.insert(Category::Files, hits);
            }
        } else {
            if self.category.includes(Category::Files) {
                self.searches.push(self.files(query.clone(), cx));
            }
            if self.category.includes(Category::Actions) {
                self.searches.push(self.actions(&query, cx));
            }
            if self.category.includes(Category::Symbols) {
                self.searches.push(self.symbols(query.clone(), cx));
            }
            if self.category.includes(Category::Text) {
                self.searches.push(self.text(query, cx));
            }
        }
        Task::ready(())
    }
    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some((category, Some(index))) = self.rows().nth(self.selected_index()) else {
            return;
        };
        let Some(hit) = self
            .results
            .get(&category)
            .and_then(|group| group.get(index))
        else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        cx.emit(DismissEvent);
        match hit {
            Hit::File(path) => workspace
                .update(cx, |workspace, cx| {
                    workspace.open_path(path.clone(), None, true, window, cx)
                })
                .detach_and_notify_err(self.workspace.clone(), window, cx),
            Hit::Action(index) => {
                if let Some((_, action)) = self.commands.get(*index) {
                    window.focus(&self.origin, cx);
                    window.dispatch_action(action.boxed_clone(), cx);
                }
            }
            Hit::Text(hit) => open_range(
                &workspace,
                hit.buffer.clone(),
                hit.anchor_range.clone(),
                window,
                cx,
            ),
            Hit::Symbol(symbol) => {
                let point = symbol.range.start;
                let buffer = self
                    .project
                    .update(cx, |project, cx| project.open_buffer_for_symbol(symbol, cx));
                window
                    .spawn(cx, async move |cx| {
                        let buffer = buffer.await?;
                        cx.update(|window, cx| {
                            let buf = buffer.read(cx);
                            let anchor = buf.anchor_before(buf.clip_point_utf16(point, Bias::Left));
                            open_range(&workspace, buffer, anchor..anchor, window, cx);
                        })
                    })
                    .detach_and_notify_err(self.workspace.clone(), window, cx);
            }
        }
    }
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let (category, index) = self.rows().nth(ix)?;
        let row = ListItem::new(ix)
            .inset(true)
            .spacing(ListItemSpacing::Sparse)
            .toggle_state(selected);
        let Some(index) = index else {
            return Some(
                row.child(
                    Label::new(if self.searches.is_empty() {
                        "Recent files"
                    } else {
                        category.label()
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
            );
        };
        let hit = self.results.get(&category)?.get(index)?;
        let line = h_flex()
            .w_full()
            .justify_between()
            .gap_3()
            .overflow_hidden();
        let detail = |label: String| {
            Label::new(label)
                .size(LabelSize::Small)
                .color(Color::Muted)
                .truncate()
        };
        Some(
            row.child(match hit {
                Hit::File(path) => line.child(Label::new(self.path_label(path, cx)).truncate()),
                Hit::Symbol(symbol) => {
                    let path = match &symbol.path {
                        SymbolLocation::InProject(path) => self.path_label(path, cx),
                        SymbolLocation::OutsideProject { abs_path, .. } => {
                            abs_path.display().to_string()
                        }
                    };
                    line.child(Label::new(symbol.label.text.clone()).truncate())
                        .child(detail(format!(
                            "{:?} · {}:{}",
                            symbol.kind,
                            path,
                            symbol.range.start.0.row + 1
                        )))
                }
                Hit::Action(index) => {
                    let (name, action) = self.commands.get(*index)?;
                    line.child(Label::new(name.clone()).truncate())
                        .child(KeyBinding::for_action_in(&**action, &self.origin, cx))
                }
                Hit::Text(hit) => line
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(render_matched_line(hit, cx)),
                    )
                    .child(detail(format!(
                        "{}:{}",
                        self.path_label(&hit.path, cx),
                        hit.line_number
                    ))),
            }),
        )
    }
    fn render_footer(&self, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        Some(
            h_flex()
                .gap_1()
                .px_2()
                .py_1()
                .children(Category::TABS.map(|tab| {
                    Button::new(tab.label(), tab.label())
                        .toggle_state(self.category == tab)
                        .on_click(cx.listener(move |picker, _, window, cx| {
                            select_category(picker, tab, window, cx)
                        }))
                }))
                .into_any_element(),
        )
    }
}

fn open_range(
    workspace: &Entity<Workspace>,
    buffer: Entity<Buffer>,
    range: Range<Anchor>,
    window: &mut Window,
    cx: &mut App,
) {
    let editor = workspace.update(cx, |workspace, cx| {
        workspace.open_project_item::<Editor>(None, buffer, true, true, true, true, window, cx)
    });
    editor.update(cx, |editor, cx| {
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        if let (Some(start), Some(end)) = (
            snapshot.anchor_in_buffer(range.start),
            snapshot.anchor_in_buffer(range.end),
        ) {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()).unfold(),
                window,
                cx,
                |selections| selections.select_ranges([start..end]),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use language::{FakeLspAdapter, Language, LanguageConfig, LanguageMatcher};
    use project::FakeFs;
    use serde_json::json;
    use settings::{BaseKeymap, DEFAULT_KEYMAP_PATH, KeymapFile, SettingsStore};
    use util::path;
    use workspace::MultiWorkspace;

    fn shift_tap(cx: &mut VisualTestContext) {
        cx.simulate_modifiers_change(Modifiers {
            shift: true,
            ..Default::default()
        });
        cx.simulate_modifiers_change(Modifiers::default());
    }

    fn open_picker(
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<Delegate>> {
        shift_tap(cx);
        shift_tap(cx);
        workspace.read_with(cx, |workspace, cx| {
            workspace
                .active_modal::<SearchEverywhere>(cx)
                .expect("double Shift opens Search Everywhere")
                .read(cx)
                .picker
                .clone()
        })
    }

    fn enter_query(picker: &Entity<Picker<Delegate>>, query: &str, cx: &mut VisualTestContext) {
        picker.update_in(cx, |picker, window, cx| picker.set_query(query, window, cx));
        cx.run_until_parked();
    }

    fn finish_search(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(DEBOUNCE);
        cx.run_until_parked();
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            crate::init(cx);
            command_palette::init(cx);
            let bindings =
                KeymapFile::load_asset_allow_partial_failure(DEFAULT_KEYMAP_PATH, cx).unwrap();
            cx.bind_keys(bindings);
            let overrides = KeymapFile::load_asset_allow_partial_failure(
                settings::SPECIFIC_OVERRIDES_KEYMAP_PATH,
                cx,
            )
            .unwrap();
            cx.bind_keys(overrides);
        });
    }

    #[gpui::test]
    async fn search_everywhere_navigation(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let source = "class InspectionDemo {\n    void RefreshTarget() {}\n}\n";
        fs.insert_tree(
            path!("/dir"),
            json!({
                ".git": {}, ".gitignore": "Generated/\n",
                "InspectionDemo.cs": source,
                "Generated": { "InspectionDemo.g.cs": "class InspectionDemo {}" },
                "Scene.unity": "InspectionDemo"
            }),
        )
        .await;
        let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
        let languages = project.read_with(cx, |project, _| project.languages().clone());
        languages.add(Arc::new(Language::new(
            LanguageConfig {
                name: "CSharp".into(),
                matcher: LanguageMatcher {
                    path_suffixes: vec!["cs".into()],
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            },
            None,
        )));
        let mut servers = languages.register_fake_lsp(
            "CSharp",
            FakeLspAdapter {
                capabilities: lsp::ServerCapabilities {
                    workspace_symbol_provider: Some(lsp::OneOf::Left(true)),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let _buffer = project
            .update(cx, |project, cx| {
                project.open_local_buffer_with_lsp(path!("/dir/InspectionDemo.cs"), cx)
            })
            .await
            .unwrap();
        let server = servers.next().await.unwrap();
        server.set_request_handler::<lsp::WorkspaceSymbolRequest, _, _>(|params, _| async move {
            #[allow(deprecated)]
            let symbol = lsp::SymbolInformation {
                name: "RefreshTarget".into(),
                kind: lsp::SymbolKind::METHOD,
                tags: None,
                deprecated: None,
                container_name: None,
                location: lsp::Location::new(
                    lsp::Uri::from_file_path(path!("/dir/InspectionDemo.cs")).unwrap(),
                    lsp::Range::new(lsp::Position::new(1, 9), lsp::Position::new(1, 22)),
                ),
            };
            Ok(Some(lsp::WorkspaceSymbolResponse::Flat(
                if params.query == "RefreshTarget" {
                    vec![symbol]
                } else {
                    vec![]
                },
            )))
        });

        let (multi, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi.read_with(cx, |multi, _| multi.workspace().clone());
        let path = project.read_with(cx, |p, cx| {
            p.find_project_path("InspectionDemo.cs", cx).unwrap()
        });
        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_path(path, None, true, window, cx)
            })
            .await
            .unwrap();
        cx.run_until_parked();
        let editor = workspace.read_with(cx, |workspace, cx| {
            workspace.active_item_as::<Editor>(cx).unwrap()
        });
        shift_tap(cx);
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert!(workspace.read_with(cx, |workspace, cx| {
            workspace.active_modal::<SearchEverywhere>(cx).is_none()
        }));
        let picker = open_picker(&workspace, cx);
        picker.read_with(cx, |picker, _| {
            assert!(matches!(
                picker.delegate.results[&Category::Files].first(),
                Some(Hit::File(_))
            ))
        });
        enter_query(&picker, "OldQuery", cx);
        let old_generation = picker.read_with(cx, |picker, _| picker.delegate.generation);
        enter_query(&picker, "InspectionDemo", cx);
        finish_search(cx);
        picker.read_with(cx, |picker, _| {
            assert_eq!(picker.delegate.results[&Category::Files].len(), 1);
            assert_eq!(picker.delegate.results[&Category::Text].len(), 2);
        });
        picker.update_in(cx, |picker, window, cx| {
            let ix = picker
                .delegate
                .rows()
                .position(|row| row == (Category::Text, Some(1)))
                .unwrap();
            picker.set_selected_index(ix, None, true, window, cx);
        });
        let mut async_cx = cx.to_async();
        let path = project.read_with(cx, |project, cx| {
            project.find_project_path("Scene.unity", cx).unwrap()
        });
        let generation = picker.read_with(cx, |p, _| p.delegate.generation);
        Delegate::publish(
            &picker.downgrade(),
            old_generation,
            Category::Files,
            vec![Hit::File(path.clone())],
            &mut async_cx,
        );
        Delegate::publish(
            &picker.downgrade(),
            generation,
            Category::Files,
            vec![Hit::File(path)],
            &mut async_cx,
        );
        picker.read_with(cx, |picker, _| {
            let delegate = &picker.delegate;
            assert_eq!(delegate.results[&Category::Files].len(), 2);
            assert_eq!(
                delegate.rows().nth(delegate.selected_index()),
                Some((Category::Text, Some(1)))
            );
        });
        cx.simulate_keystrokes("tab");
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.delegate.category, Category::Files);
            assert_eq!(picker.query(cx), "InspectionDemo");
        });
        cx.simulate_keystrokes("enter");
        assert!(workspace.read_with(cx, |workspace, cx| {
            workspace.active_modal::<SearchEverywhere>(cx).is_none()
        }));

        // The optional preset overlays the platform defaults in this order in Zed.
        cx.update(|_, cx| {
            let bindings = KeymapFile::load_asset_allow_partial_failure(
                BaseKeymap::JetBrains.asset_path().unwrap(),
                cx,
            )
            .unwrap();
            cx.bind_keys(bindings);
            let overrides = KeymapFile::load_asset_allow_partial_failure(
                settings::SPECIFIC_OVERRIDES_KEYMAP_PATH,
                cx,
            )
            .unwrap();
            cx.bind_keys(overrides);
        });
        let picker = open_picker(&workspace, cx);
        enter_query(&picker, "RefreshTarget", cx);
        cx.simulate_keystrokes("tab tab");
        finish_search(cx);
        picker.read_with(cx, |picker, _| {
            assert!(matches!(
                picker.delegate.results[&Category::Symbols].first(),
                Some(Hit::Symbol(_))
            ))
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            assert_eq!(
                editor.selections.newest::<text::Point>(&snapshot).head(),
                text::Point::new(1, 9)
            );
        });

        let picker = open_picker(&workspace, cx);
        enter_query(&picker, "RefreshTarget()", cx);
        cx.simulate_keystrokes("shift-tab");
        finish_search(cx);
        cx.simulate_keystrokes("enter");
        editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            let range = editor
                .selections
                .newest::<multi_buffer::MultiBufferOffset>(&snapshot)
                .range();
            assert_eq!(&source[range.start.0..range.end.0], "RefreshTarget()");
        });

        let picker = open_picker(&workspace, cx);
        enter_query(&picker, "editor::SelectAll", cx);
        cx.simulate_keystrokes("shift-tab shift-tab");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            let range = editor
                .selections
                .newest::<multi_buffer::MultiBufferOffset>(&snapshot)
                .range();
            assert_eq!(range.start.0..range.end.0, 0..source.len());
        });
        open_picker(&workspace, cx);
        cx.simulate_keystrokes("escape");
        editor.update_in(cx, |editor, window, cx| {
            assert!(editor.focus_handle(cx).is_focused(window))
        });
    }
    #[gpui::test]
    async fn search_everywhere_keymap_overrides(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(path!("/demo"), json!({ "Test.txt": "Sample" }))
            .await;
        let project = Project::test(fs, [path!("/demo").as_ref()], cx).await;
        let (multi, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi.read_with(cx, |multi, _| multi.workspace().clone());
        let path = project.read_with(cx, |p, cx| p.find_project_path("Test.txt", cx).unwrap());
        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_path(path, None, true, window, cx)
            })
            .await
            .unwrap();
        cx.run_until_parked();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        cx.simulate_modifiers_change(shift);
        cx.simulate_keystrokes("shift-right");
        cx.simulate_modifiers_change(Modifiers::default());
        shift_tap(cx);
        assert!(workspace.read_with(cx, |w, cx| w.active_modal::<SearchEverywhere>(cx).is_none()));
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        let picker = open_picker(&workspace, cx);
        enter_query(&picker, "Sample", cx);
        finish_search(cx);
        picker.read_with(cx, |picker, _| {
            assert!(!picker.delegate.results.contains_key(&Category::Symbols));
            assert_eq!(picker.delegate.results[&Category::Text].len(), 1);
        });
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            let source = settings::KeybindSource::User.meta();
            cx.bind_keys([
                gpui::KeyBinding::new("shift shift", gpui::NoAction, Some("Workspace"))
                    .with_meta(source),
                gpui::KeyBinding::new(
                    "ctrl-alt-shift-j",
                    ToggleSearchEverywhere,
                    Some("Workspace"),
                )
                .with_meta(source),
            ]);
        });
        shift_tap(cx);
        shift_tap(cx);
        assert!(workspace.read_with(cx, |w, cx| w.active_modal::<SearchEverywhere>(cx).is_none()));
        cx.simulate_keystrokes("ctrl-alt-shift-j");
        assert!(workspace.read_with(cx, |w, cx| w.active_modal::<SearchEverywhere>(cx).is_some()));
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-shift-p"
        } else {
            "ctrl-shift-p"
        });
        assert!(workspace.read_with(cx, |w, cx| {
            w.active_modal::<command_palette::CommandPalette>(cx)
                .is_some()
        }));
    }
}

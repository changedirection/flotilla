mod actions;
mod app;
mod data;
mod event;
mod template;
mod ui;
mod config;

use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;
use clap::Parser;
use color_eyre::Result;
use crossterm::{execute, event::{EnableMouseCapture, DisableMouseCapture}};

/// TUI dashboard for managing development workspaces across cmux, worktrunk, and GitHub.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Git repo roots (repeatable; auto-detected from cwd if omitted)
    #[arg(long)]
    repo_root: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    // Collect repos: CLI args first, then persisted, then auto-detect
    let mut repo_roots: Vec<PathBuf> = Vec::new();
    for root in &cli.repo_root {
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        repo_roots.push(canonical);
    }

    // Auto-detect from cwd if no CLI args
    if repo_roots.is_empty() {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                repo_roots.push(path);
            }
        }
    }

    // Load persisted repos
    let persisted = config::load_repos();
    for path in persisted {
        if !repo_roots.contains(&path) {
            repo_roots.push(path);
        }
    }

    // Persist any new CLI repos
    for path in &repo_roots {
        config::save_repo(path);
    }

    if repo_roots.is_empty() {
        eprintln!("Error: no git repositories found (use --repo-root to specify)");
        std::process::exit(1);
    }

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let result = run(&mut terminal, repo_roots).await;
    execute!(stdout(), DisableMouseCapture)?;
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal, repo_roots: Vec<PathBuf>) -> Result<()> {
    let mut app = app::App::new(repo_roots);
    let mut events = event::EventHandler::new(Duration::from_millis(250));
    let mut last_refresh = std::time::Instant::now();
    let refresh_interval = Duration::from_secs(10);

    // Initial data load — all repos in parallel
    refresh_all(&mut app).await;

    loop {
        terminal.draw(|f| ui::render(&mut app, f))?;

        if let Some(evt) = events.next().await {
            match evt {
                event::Event::Key(k) => {
                    if k.code == crossterm::event::KeyCode::Char('r')
                        && !app.show_action_menu
                        && app.input_mode == app::InputMode::Normal
                        && !app.show_help
                        && !app.show_delete_confirm
                    {
                        refresh_all(&mut app).await;
                        last_refresh = std::time::Instant::now();
                    } else {
                        app.handle_key(k);
                    }
                }
                event::Event::Mouse(m) => {
                    // Check for tab clicks
                    if m.kind == crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) {
                        let x = m.column;
                        let y = m.row;
                        let mut tab_clicked = false;
                        // Check tab areas
                        for (i, tab_area) in app.tab_areas.iter().enumerate() {
                            if x >= tab_area.x && x < tab_area.x + tab_area.width
                                && y >= tab_area.y && y < tab_area.y + tab_area.height
                            {
                                app.switch_tab(i);
                                tab_clicked = true;
                                break;
                            }
                        }
                        if !tab_clicked {
                            // Check [+] button
                            let a = app.add_tab_area;
                            if x >= a.x && x < a.x + a.width && y >= a.y && y < a.y + a.height {
                                // Open file picker via keyboard shortcut simulation
                                app.input_mode = app::InputMode::AddRepo;
                                app.input.reset();
                                if let Some(parent) = app.active_repo_root().parent() {
                                    let parent_str = format!("{}/", parent.display());
                                    app.input = tui_input::Input::from(parent_str.as_str());
                                }
                                app.dir_entries = Vec::new();
                                app.dir_selected = 0;
                                app.refresh_dir_listing();
                                tab_clicked = true;
                            }
                        }
                        if !tab_clicked {
                            app.handle_mouse(m);
                        }
                    } else {
                        app.handle_mouse(m);
                    }
                }
                event::Event::Tick => {
                    if last_refresh.elapsed() >= refresh_interval {
                        refresh_all(&mut app).await;
                        last_refresh = std::time::Instant::now();
                    }
                }
            }
        }

        // Process pending actions — clear status only when user triggers an action
        let pending = app.take_pending_action();
        if !matches!(pending, app::PendingAction::None) {
            app.status_message = None;
        }
        match pending {
            app::PendingAction::SwitchWorktree(i) => {
                if let Some(wt) = app.active().data.worktrees.get(i).cloned() {
                    let tmpl = template::WorkspaceTemplate::load(app.active_repo_root());
                    if let Err(e) = actions::create_cmux_workspace(
                        &tmpl,
                        &wt.path,
                        "claude",
                        &wt.branch,
                    ).await {
                        app.status_message = Some(e);
                    }
                    refresh_all(&mut app).await;
                }
            }
            app::PendingAction::SelectWorkspace(ws_ref) => {
                if let Err(e) = actions::select_cmux_workspace(&ws_ref).await {
                    app.status_message = Some(e);
                }
            }
            app::PendingAction::FetchDeleteInfo(si) => {
                let table_idx = app.active().data.selectable_indices.get(si).copied();
                if let Some(table_idx) = table_idx {
                    if let Some(data::TableEntry::Item(item)) = app.active().data.table_entries.get(table_idx).cloned() {
                        let branch = item.branch.clone().unwrap_or_default();
                        let wt_path = item.worktree_idx
                            .and_then(|idx| app.active().data.worktrees.get(idx))
                            .map(|wt| wt.path.clone());
                        let pr_number = item.pr_idx
                            .and_then(|idx| app.active().data.prs.get(idx))
                            .map(|pr| pr.number);
                        let repo_root = app.active_repo_root().clone();
                        let info = data::fetch_delete_confirm_info(
                            &branch,
                            wt_path.as_ref(),
                            pr_number,
                            &repo_root,
                        ).await;
                        app.delete_confirm_info = Some(info);
                        app.delete_confirm_loading = false;
                    }
                }
            }
            app::PendingAction::ConfirmDelete => {
                if let Some(info) = app.delete_confirm_info.take() {
                    let repo = app.active_repo_root().clone();
                    if let Err(e) = actions::remove_worktree(&info.branch, &repo).await {
                        app.status_message = Some(e);
                    }
                    refresh_all(&mut app).await;
                }
            }
            app::PendingAction::OpenPr(number) => {
                let repo = app.active_repo_root().clone();
                let _ = actions::open_pr_in_browser(number, &repo).await;
            }
            app::PendingAction::OpenIssueBrowser(number) => {
                let repo = app.active_repo_root().clone();
                let _ = actions::open_issue_in_browser(number, &repo).await;
            }
            app::PendingAction::CreateWorktree(branch) => {
                let repo = app.active_repo_root().clone();
                match actions::create_worktree(&branch, &repo).await {
                    Ok(wt_path) => {
                        let tmpl = template::WorkspaceTemplate::load(app.active_repo_root());
                        if let Err(e) = actions::create_cmux_workspace(
                            &tmpl,
                            &wt_path,
                            "claude",
                            &branch,
                        ).await {
                            app.status_message = Some(e);
                        }
                    }
                    Err(e) => app.status_message = Some(e),
                }
                refresh_all(&mut app).await;
            }
            app::PendingAction::ArchiveSession(ses_idx) => {
                if let Some(session) = app.active().data.sessions.get(ses_idx).cloned() {
                    if let Err(e) = data::archive_session(&session.id).await {
                        app.status_message = Some(e);
                    }
                    refresh_all(&mut app).await;
                }
            }
            app::PendingAction::TeleportSession { session_id, branch, worktree_idx } => {
                let teleport_cmd = format!("claude --teleport {}", session_id);
                let tmpl = template::WorkspaceTemplate::load(app.active_repo_root());
                let wt_path = if let Some(wt_idx) = worktree_idx {
                    app.active().data.worktrees.get(wt_idx).map(|wt| wt.path.clone())
                } else if let Some(branch_name) = &branch {
                    let repo = app.active_repo_root().clone();
                    actions::create_worktree(branch_name, &repo).await.ok()
                } else {
                    None
                };
                if let Some(path) = wt_path {
                    let name = branch.as_deref().unwrap_or("session");
                    if let Err(e) = actions::create_cmux_workspace(
                        &tmpl, &path, &teleport_cmd, name,
                    ).await {
                        app.status_message = Some(e);
                    }
                }
                refresh_all(&mut app).await;
            }
            app::PendingAction::GenerateBranchName(issue_idxs) => {
                let issues: Vec<(i64, String)> = issue_idxs
                    .iter()
                    .filter_map(|&idx| app.active().data.issues.get(idx))
                    .map(|issue| (issue.number, issue.title.clone()))
                    .collect();
                let repo = app.active_repo_root().clone();
                let issue_refs: Vec<(i64, &str)> = issues.iter().map(|(n, t)| (*n, t.as_str())).collect();
                match actions::generate_branch_name(&issue_refs, &repo).await {
                    Ok(branch) => app.prefill_branch_input(&branch),
                    Err(_) => {
                        let fallback: Vec<String> = issues
                            .iter()
                            .map(|(num, _)| format!("issue-{}", num))
                            .collect();
                        app.prefill_branch_input(&fallback.join("-"));
                    }
                }
            }
            app::PendingAction::AddRepo(path) => {
                config::save_repo(&path);
                app.add_repo(path);
                app.switch_tab(app.repo_order.len() - 1);
                refresh_all(&mut app).await;
            }
            app::PendingAction::None => {}
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn refresh_all(app: &mut app::App) {
    // Snapshot all repos for change detection
    let snapshots: Vec<_> = app.repo_order.iter()
        .map(|path| app.repos[path].data_snapshot())
        .collect();

    // Refresh all repos in parallel
    let data_stores: Vec<_> = app.repo_order.iter()
        .map(|path| {
            let mut ds = std::mem::take(&mut app.repos.get_mut(path).unwrap().data);
            let root = path.clone();
            async move {
                let errors = ds.refresh(&root).await;
                (root, ds, errors)
            }
        })
        .collect();

    let results = futures::future::join_all(data_stores).await;

    let mut all_errors: Vec<String> = Vec::new();
    for (i, (path, data, errors)) in results.into_iter().enumerate() {
        let rs = app.repos.get_mut(&path).unwrap();
        rs.data = data;

        // Change detection
        let new_snapshot = rs.data_snapshot();
        if snapshots[i] != new_snapshot && i != app.active_repo {
            rs.has_unseen_changes = true;
        }

        // Restore selection
        if rs.data.selectable_indices.is_empty() {
            rs.selected_selectable_idx = None;
            rs.table_state.select(None);
        } else if rs.selected_selectable_idx.is_none() {
            rs.selected_selectable_idx = Some(0);
            rs.table_state.select(Some(rs.data.selectable_indices[0]));
        } else if let Some(si) = rs.selected_selectable_idx {
            let clamped = si.min(rs.data.selectable_indices.len() - 1);
            rs.selected_selectable_idx = Some(clamped);
            rs.table_state.select(Some(rs.data.selectable_indices[clamped]));
        }

        // Collect errors with repo name prefix
        if !errors.is_empty() {
            let name = app::App::repo_name(&path);
            for e in errors {
                all_errors.push(format!("{name}: {e}"));
            }
        }
    }

    if !all_errors.is_empty() {
        app.status_message = Some(all_errors.join("; "));
    }
}

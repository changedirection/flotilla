# Multi-Repo Support

## Context

The app currently manages a single git repository. Users working across multiple repos must run separate instances. This design adds tabbed multi-repo support with persistence, parallel refresh, change indicators, and an inline file picker for adding repos.

## Data Model

```rust
struct RepoState {
    data: DataStore,
    table_state: TableState,
    selected_selectable_idx: Option<usize>,
    has_unseen_changes: bool,
}
```

On `App`:
- Remove `repo_root: PathBuf`, `data: DataStore`, `table_state: TableState`, `selected_selectable_idx: Option<usize>`
- Add `repos: HashMap<PathBuf, RepoState>` — keyed by absolute repo path
- Add `repo_order: Vec<PathBuf>` — tab ordering, reorderable via `swap()`
- Add `active_repo: usize` — index into `repo_order`

Helper methods `active() -> &RepoState` and `active_mut() -> &mut RepoState` resolve the indirection: `&self.repos[&self.repo_order[self.active_repo]]`.

### Change Detection

Before each refresh, snapshot `(worktrees.len(), prs.len(), sessions.len(), remote_branches.len(), issues.len())` per repo. After refresh, if the tuple differs and the repo is not the active tab, set `has_unseen_changes = true`. Switching to a tab clears it.

## Tab Bar

The title bar becomes a tab bar. Same single row:

```
 cmux-controller  | scratch | reticulate | other-repo* | [+]
```

- Active tab: bold cyan. Inactive: dim.
- `*` suffix on tabs with `has_unseen_changes`.
- `[+]` button at the right end — clickable, opens file picker.
- `[` / `]` keys switch tabs. Mouse click on a tab switches to it.

## Config Persistence

Directory: `~/.config/cmux-controller/repos/`

Each repo gets a file named by a slug of its absolute path (path separators replaced with dashes, leading dash stripped):

```
~/.config/cmux-controller/repos/users-robert-dev-scratch.toml
```

Contents:

```toml
path = "/Users/robert/dev/scratch"
```

### Lifecycle

- **Startup**: scan `repos/` dir, load all paths into `repos` HashMap + `repo_order`.
- **CLI `--repo-root` args**: add to the map if not present, create the toml file to persist.
- **File picker `[+]`**: validates it's a git repo, adds + persists.
- **Remove tab** (future): deletes the toml file.

### Ordering

`repo_order` is built from filesystem scan order on startup (alphabetical by slug). CLI args appear first, then persisted. Tab reordering is session-only (persisting order is deferred).

## Refresh & Parallelism

All repos refresh in parallel on every 10s tick:

```rust
async fn refresh_all(&mut self) -> Vec<String> {
    let futures = self.repo_order.iter().map(|path| {
        // each calls DataStore::refresh() internally
    });
    let results = futures::future::join_all(futures).await;
    // compare snapshots, set has_unseen_changes, collect errors
}
```

Each repo's `DataStore::refresh()` already runs its fetches in parallel via `tokio::join!`. The outer `join_all` parallelizes across repos. Errors are prefixed with the repo name: `"scratch: sessions: token expired"`.

## File Picker

Triggered by clicking `[+]` or pressing `a` (add). Opens a popup:

```
+-- Add Repository ---------------------+
| > /Users/robert/dev/                  |
|                                       |
|   other-project/          (git repo)  |
|   scratch/                (added)     |
|   reticulate/             (git repo)  |
|   notes/                              |
+---------------------------------------+
```

- Starts with parent directory of the active repo.
- Directory listing updates as you type (synchronous `std::fs::read_dir`).
- Git repos tagged `(git repo)`; already-added repos tagged `(added)`.
- j/k or arrow keys to select from the list, Tab to complete into input.
- Enter on a git repo adds it. Enter on a plain directory descends into it.
- Reuses existing popup + `tui_input` pattern.

## CLI Changes

```rust
#[derive(Parser)]
struct Cli {
    /// Git repo roots (repeatable; auto-detected from cwd if omitted)
    #[arg(long)]
    repo_root: Vec<PathBuf>,
}
```

- No args: auto-detect from cwd, load persisted repos.
- With args: each arg gets added to persisted config, other persisted repos also load.
- Auto-detected or first CLI repo is the initially active tab.

## Files to Modify

| File | Changes |
|------|---------|
| `app.rs` | `RepoState` struct. Replace single-repo fields with `repos` HashMap + `repo_order` + `active_repo`. Add `active()`/`active_mut()`. Tab switching on `[`/`]`. File picker input mode. |
| `data.rs` | No structural changes. `DataStore` and `refresh()` remain per-repo. |
| `ui.rs` | Title bar becomes tab bar. File picker popup. Change indicator on tabs. Mouse hit-testing for tab clicks and `[+]`. |
| `main.rs` | Config loading/persisting on startup. `refresh_all()` replaces single `refresh_data()`. Plural `--repo-root`. |
| `config.rs` (new) | Config directory management: scan, load, save, slug generation. |

## What Stays Unchanged

- `actions.rs` — all actions already take `repo_root` as a parameter
- `template.rs` — already takes `repo_root` as a parameter
- `event.rs` — event handling is repo-agnostic
- `data.rs` — `DataStore` is already per-repo scoped

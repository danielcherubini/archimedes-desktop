pub mod agent;
pub mod agents;
pub mod commands;
pub mod config;
pub mod skills;
pub mod storage;
#[doc(hidden)]
pub mod test_support;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::Manager;

use crate::agent::worker::client::WorkerHandle;
use crate::agent::worker::manager::{WorkerFactory, WorkerManager};
use crate::agent::{crashlog, debuglog, EventSink, SessionManager, SubagentSessionManager};
use crate::storage::Db;

/// The production `WorkerFactory` (the `current_exe()` Worker — the
/// `--worker` mode of this same binary; the `WorkerHandle`'s spawn
/// passes the flag). Tests inject the `fake_worker` fixture instead
/// (the `setup_dirs_with_factory` seam).
struct ProductionWorkerFactory;

impl WorkerFactory for ProductionWorkerFactory {
    fn spawn(&self) -> Result<WorkerHandle, crate::agent::worker::client::WorkerError> {
        let exe = std::env::current_exe().expect("the current exe (the Worker binary)");
        WorkerHandle::spawn(exe.as_path())
    }
}

/// The shared app setup (the `test_support` seam): the session manager
/// (settings-driven native sessions), the persistence database in
/// `app_data_dir/archimedes.db`, and the `WorkerManager` (ADR 0025 — the
/// session's `AgentLoop` runs in the Worker; the `SessionManager` is the
/// Supervisor-side coordinator).
///
/// Factored out of [`run`] so the real command surface can be exercised in
/// tests without a GUI (see `tests/ipc.rs`). The `WorkerFactory` is the
/// PRODUCTION one (`current_exe()`); the test variant is
/// [`setup_dirs_with_factory`] (the injectable factory — the `test_support`
/// pattern).
pub fn setup_dirs<R: tauri::Runtime>(
    app: &tauri::App<R>,
    config_dir: PathBuf,
    app_data_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    setup_dirs_with_factory(app, config_dir, app_data_dir, None)
}

/// The `setup_dirs` seam (the `test_support` pattern — the injectable
/// `WorkerFactory`): `None` = the production factory (`current_exe()`);
/// `Some(factory)` = the test factory (the `fake_worker` fixture — the
/// `tests/ipc.rs` headless command-surface test). Everything else is
/// IDENTICAL to [`setup_dirs`].
pub fn setup_dirs_with_factory<R: tauri::Runtime>(
    app: &tauri::App<R>,
    config_dir: PathBuf,
    app_data_dir: PathBuf,
    worker_factory: Option<Arc<dyn WorkerFactory>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The session manager is settings-driven (the config dir is the
    // `settings.json` home — the desktop is native-only: there is no agent
    // registry and no pi config seeding).
    let mut manager = SessionManager::new(config_dir.clone());
    // The persistence database lives in the app data directory.
    let db = Arc::new(
        Db::open(&app_data_dir.join("archimedes.db"))
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
    );
    manager.attach_db(db.clone());
    // The subagent manager (the ADR 0025 Task 5 re-plumb — the
    // in-process subagent machinery is GONE: the dispatch runs in a
    // WORKER via the `WorkerManager`'s `dispatch_subagent` flow — the
    // `WorkerManager` is forwarded by `attach_worker_manager` (the
    // `set_worker_manager` + the child-cwd registrar); the manager is NO
    // LONGER managed Tauri state (the `respond_*` commands route through
    // the `SessionManager`'s `WorkerManager` `handle_for` — a subagent
    // session's handle is in the `drives` map)).
    manager.set_subagent_manager(Arc::new(SubagentSessionManager::new()));
    // The event sink (TauriSink) is managed state so commands — and the
    // headless IPC test — can obtain it without an `AppHandle` parameter.
    //
    // It must be managed under the TRAIT-OBJECT type `Arc<dyn EventSink>`:
    // the commands resolve it as `State<Arc<dyn EventSink>>` and Tauri's
    // state registry is keyed by TypeId — managing the concrete
    // `Arc<TauriSink<R>>` registers a different key, and `start_session`
    // fails at runtime with "state not managed for field `sink`".
    // (`Arc::<dyn EventSink>::new` doesn't compile because `dyn EventSink`
    // is unsized, so the coercion is spelled as an annotated binding.)
    let sink: Arc<dyn EventSink> = Arc::new(commands::sessions::TauriSink(app.handle().clone()));
    app.manage(sink.clone());
    // The router's `SinkFrame` re-emit target (the `setup_dirs` late-wire
    // — set BEFORE the `WorkerManager`'s callbacks can fire).
    manager.set_sink(sink.clone());
    // The `WorkerManager` (ADR 0025 — the session's `AgentLoop` runs in
    // the Worker): the LATE-WIRE (the `SessionManager` ↔ `WorkerManager`
    // construction cycle — the `WorkerManager`'s `on_event` / `on_crash`
    // callbacks need the `SessionManager`'s router / crash handler, and
    // the `SessionManager` needs the `WorkerManager` to send prompts):
    // the callbacks capture the `Arc<SessionManager>` (the `attach_*`
    // methods are `&self` — the `OnceLock` `set` needs no exclusive
    // access), then `attach_worker_manager` hands the `WorkerManager`
    // to the `SessionManager` after both are built.
    let manager = Arc::new(manager);
    let wm = {
        let m = manager.clone();
        let m2 = manager.clone();
        let sink_for = sink.clone();
        Arc::new(WorkerManager::new(
            worker_factory.unwrap_or_else(|| Arc::new(ProductionWorkerFactory)),
            Arc::new(move |session_id, code| m.handle_crash(&session_id, code)),
            Arc::new(move |session_id, is_subagent, evt| {
                m2.route_event(&session_id, is_subagent, evt)
            }),
            // The parent-scope sink lookup (the subagent UI lifecycle
            // events — the single shared `TauriSink`).
            Arc::new(move |_| Some(sink_for.clone())),
        ))
    };
    manager.attach_worker_manager(wm.clone());
    // The `WorkerManager` is managed state (the `set_space_trusted`
    // command's running-Workers `Config` push + the `test_support`
    // registry accessor).
    app.manage(wm);
    // The manager is `Sync` (its mutable state is `Arc<Mutex<…>>`
    // internally), so it is shared directly without an outer lock.
    app.manage(manager);
    app.manage(db);
    Ok(())
}

/// The Worker mode (ADR 0025): a fresh tokio runtime + the stdio JSONL
/// worker (`agent::worker::run_worker_inner`). Checked BEFORE Tauri
/// init in `main` — the Tauri runtime never starts in a Worker.
pub fn run_worker() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the worker tokio runtime");
    runtime.block_on(agent::worker::run_worker_inner());
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The Supervisor's crash hook (ADR 0025 Task 6 — the Task-2 shared
    // hook, tag `"supervisor"`): a Supervisor panic STILL kills the app
    // (it IS the app — the `release` profile's `panic = "unwind"` only
    // protects the WORKERS), but a `crash-<ts>-supervisor.log` now
    // exists; the live Workers exit cleanly on the stdin EOF (the
    // `worker_smoke` test's EOF case). The `ARCHIMEDES_DEBUG` init
    // reads the var ONCE (the cached check — `log` is a free no-op when
    // unset).
    crashlog::install_panic_hook("supervisor");
    debuglog::init_from_env();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::sessions::start_session,
            commands::sessions::send_prompt,
            commands::sessions::close_session,
            commands::sessions::respond_permission,
            commands::sessions::respond_interactive_request,
            commands::sessions::resume_session,
            commands::sessions::set_session_config_option,
            commands::sessions::cancel_session,
            commands::sessions::get_stalled_info,
            commands::history::list_sessions,
            commands::history::load_history,
            commands::history::delete_session,
            commands::history::set_session_archived,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::list_models,
            commands::settings::list_tools,
            commands::settings::list_known_providers,
            commands::settings::shell_sandbox_available,
            commands::settings::test_mcp_server,
            commands::settings::auth_mcp_server,
            commands::mcp::list_mcp_servers_effective,
            commands::agents::list_agent_definitions,
            commands::agents::list_agent_definitions_for_space,
            commands::spaces::list_spaces,
            commands::spaces::delete_space,
            commands::spaces::space_for_path,
            commands::spaces::set_space_trusted,
            commands::skills::list_skills,
            commands::clipboard::read_clipboard_image,
            commands::files::read_file_bytes,
            commands::files::list_space_files
        ])
        .setup(|app| {
            // The on-disk dirs are named `archimedes` (NOT the bundle
            // identifier — Tauri's `config_dir()` / `app_data_dir()`
            // append the identifier, which would give
            // `…/codes.archimedes.desktop`): `~/.config/archimedes` +
            // `~/.local/share/archimedes` (Linux), `~/Library/Application
            // Support/archimedes` (macOS), `%APPDATA%/archimedes` +
            // `%LOCALAPPDATA%/archimedes` (Windows). `dirs` resolves the
            // platform base dirs the same way Tauri does; a `None` base
            // means the platform home is undeterminable — a hard error
            // (the app has nowhere to persist state).
            let config_dir = dirs::config_dir()
                .map(|p| p.join("archimedes"))
                .ok_or_else(|| -> Box<dyn std::error::Error> {
                    "could not resolve the platform config dir".into()
                })?;
            let app_data_dir = dirs::data_dir().map(|p| p.join("archimedes")).ok_or_else(
                || -> Box<dyn std::error::Error> {
                    "could not resolve the platform data dir".into()
                },
            )?;
            setup_dirs(app, config_dir, app_data_dir)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    // The `run` callback form (ADR 0025 — the `RunEvent` hook): on
    // `Exit` (the event loop is exiting — the Tauri 2 name for the
    // `Exited` event; the app is about to terminate), reap the live
    // Workers (the graceful `close` + grace + `kill` — the Workers'
    // stdin EOF → clean exit 0). The `reap_all` is async (the `close`
    // + grace + `kill` select) — the `Exit` callback is NOT in a tokio
    // context (the event loop is Tauri's own), so a fresh
    // current-thread runtime drives it (a short-lived, bounded block —
    // the process is exiting anyway).
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(wm) = handle.try_state::<Arc<WorkerManager>>() {
                let wm = wm.inner().clone();
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("the exit reap runtime");
                rt.block_on(wm.reap_all());
            }
        }
    });
}

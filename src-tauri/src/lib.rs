//! Tauri shell: translates UI calls into `unlost-core` calls and streams job progress.
//!
//! This process stays a normal user. Raw-disk work goes to the elevated broker through the
//! engine; recovered files are written here, after the destination guard approved the folder.

mod recover;

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use unlost_core::disks::{self, DriveInfo};
use unlost_core::engine::{Canceller, Engine};
use unlost_core::error::ErrorPayload;
use unlost_core::guard::{self, DiskSet, Verdict};
use unlost_core::job::{Job, ProgressSnapshot};
use unlost_core::protocol::{EntryView, Page, Query, Request, ScanSummary};
use unlost_core::recycle::{self, BinItem};
use unlost_core::settings::{self, Settings};
use unlost_core::shadow::{ShadowCopy, ShadowEntry};
use unlost_core::sources::{self, SourceSpec};
use unlost_core::writer::RunSummary;
use unlost_core::{Error, system, workspace};
use unlost_formats::partition::Partition;

type CmdResult<T> = Result<T, ErrorPayload>;

fn err(e: Error) -> ErrorPayload {
    ErrorPayload::from(&e)
}

pub struct AppState {
    engine: Arc<Engine>,
    data_dir: PathBuf,
    settings: RwLock<Settings>,
    sid: String,
    cancels: Mutex<HashMap<String, Canceller>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    kind: String,
    #[serde(flatten)]
    snapshot: ProgressSnapshot,
}

fn emit(app: &AppHandle, kind: &str, snapshot: ProgressSnapshot) {
    let _ = app.emit(
        "job-progress",
        ProgressEvent {
            kind: kind.into(),
            snapshot,
        },
    );
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> CmdResult<T> {
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(r) => r.map_err(err),
        Err(e) => Err(err(Error::Other(e.to_string()))),
    }
}

/// Runs `work` as job `kind` with a cancel handle and a progress event every 150 ms.
async fn run_job<T, F>(app: &AppHandle, kind: &str, work: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<Job>) -> Result<T, Error> + Send + 'static,
{
    let state = app.state::<AppState>();
    let job = Arc::new(Job::new());
    let j = job.clone();
    state
        .cancels
        .lock()
        .insert(kind.into(), Arc::new(move || j.cancel()));
    let done = Arc::new(AtomicBool::new(false));
    {
        let (app, job, done, kind) = (app.clone(), job.clone(), done.clone(), kind.to_string());
        std::thread::spawn(move || {
            loop {
                let finished = done.load(Ordering::Relaxed);
                emit(&app, &kind, job.snapshot());
                if finished {
                    break;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        });
    }
    let r = blocking(move || work(job)).await;
    done.store(true, Ordering::Relaxed);
    state.cancels.lock().remove(kind);
    r
}

// ---------------------------------------------------------------- app

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    elevated: bool,
    portable: bool,
    admin_ready: bool,
    data_dir: String,
    /// Physical disks holding the app and its data folder: recovering from these disks means
    /// the app's own writes can land on them.
    app_disks: Vec<u32>,
}

#[tauri::command]
async fn app_info(app: AppHandle) -> AppInfo {
    let state = app.state::<AppState>();
    let data_dir = state.data_dir.clone();
    let app_disks = tauri::async_runtime::spawn_blocking(move || {
        let mut d: Vec<u32> = Vec::new();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        for p in [exe_dir, Some(data_dir)].into_iter().flatten() {
            if let Ok(set) = guard::disks_of_path(&p) {
                d.extend(set.disks);
            }
        }
        d.sort_unstable();
        d.dedup();
        d
    })
    .await
    .unwrap_or_default();
    AppInfo {
        version: app.package_info().version.to_string(),
        elevated: system::is_elevated(),
        portable: settings::is_portable(),
        admin_ready: state.engine.admin_ready(),
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        app_disks,
    }
}

#[tauri::command]
fn settings_get(state: State<'_, AppState>) -> Settings {
    state.settings.read().clone()
}

#[tauri::command]
fn settings_set(state: State<'_, AppState>, settings: Settings) -> CmdResult<Settings> {
    settings.save(&state.data_dir).map_err(err)?;
    *state.settings.write() = settings.clone();
    Ok(settings)
}

#[tauri::command]
fn job_cancel(state: State<'_, AppState>, kind: String) {
    if let Some(c) = state.cancels.lock().get(&kind) {
        c();
    }
}

/// Shows the recovery folder in Explorer. Only folders: recovered files are never opened.
#[tauri::command]
fn reveal_path(path: String) -> CmdResult<()> {
    let p = Path::new(&path);
    if !p.is_absolute() || !p.is_dir() {
        return Err(err(Error::Refused(
            "only existing folders can be shown".into(),
        )));
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("\"{}\"", p.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| err(Error::io(p, e)))
}

// ---------------------------------------------------------------- drives and outlook

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DriveView {
    #[serde(flatten)]
    info: DriveInfo,
    bin_items: u64,
    bin_bytes: u64,
}

#[tauri::command]
async fn drives(app: AppHandle) -> CmdResult<Vec<DriveView>> {
    let sid = app.state::<AppState>().sid.clone();
    blocking(move || {
        let all = disks::drives();
        let roots: Vec<String> = all.iter().map(|d| d.root.clone()).collect();
        let bin = recycle::list(&sid, &roots);
        Ok(all
            .into_iter()
            .map(|info| {
                let mine: Vec<&BinItem> = bin
                    .iter()
                    .filter(|b| b.drive == info.root && b.present)
                    .collect();
                DriveView {
                    bin_items: mine.len() as u64,
                    bin_bytes: mine.iter().map(|b| b.size).sum(),
                    info,
                }
            })
            .collect())
    })
    .await
}

/// Phones and cameras plugged in as media devices: shown so the app can explain why they are
/// not in the drive list.
#[tauri::command]
async fn media_devices() -> CmdResult<Vec<String>> {
    blocking(|| Ok(disks::media_devices())).await
}

#[tauri::command]
async fn admin_start(app: AppHandle) -> CmdResult<bool> {
    let engine = app.state::<AppState>().engine.clone();
    blocking(move || engine.start_broker()).await
}

// ---------------------------------------------------------------- recycle bin and shadow copies

#[tauri::command]
async fn recycle_list(app: AppHandle) -> CmdResult<Vec<BinItem>> {
    let sid = app.state::<AppState>().sid.clone();
    blocking(move || {
        let roots: Vec<String> = disks::drives().into_iter().map(|d| d.root).collect();
        Ok(recycle::list(&sid, &roots))
    })
    .await
}

fn call_value(engine: &Engine, req: Request, admin: bool) -> Result<serde_json::Value, Error> {
    engine.call(req, admin)?.wait(|_| {}, |_| Ok(()))
}

#[tauri::command]
async fn shadow_list(app: AppHandle) -> CmdResult<Vec<ShadowCopy>> {
    let engine = app.state::<AppState>().engine.clone();
    blocking(move || {
        Ok(serde_json::from_value(call_value(
            &engine,
            Request::ShadowList,
            true,
        )?)?)
    })
    .await
}

#[tauri::command]
async fn shadow_dir(app: AppHandle, device: String, path: String) -> CmdResult<Vec<ShadowEntry>> {
    let engine = app.state::<AppState>().engine.clone();
    blocking(move || {
        Ok(serde_json::from_value(call_value(
            &engine,
            Request::ShadowDir { device, path },
            true,
        )?)?)
    })
    .await
}

// ---------------------------------------------------------------- deleted-file scans

#[tauri::command]
async fn image_partitions(path: String) -> CmdResult<Vec<Partition>> {
    blocking(move || sources::image_partitions(Path::new(&path))).await
}

/// Starts a scan. Its session database goes to the app's data folder, or to `Unlost scan data`
/// inside `workspace` when given; either way only after the guard found the folder on none of
/// the source's disks. The UI asks for `workspace` when the data folder shares a disk with the
/// source (`workspace_same_disk`).
#[tauri::command]
async fn scan_start(
    app: AppHandle,
    source: SourceSpec,
    workspace: Option<String>,
    confirm_unknown: Option<bool>,
) -> CmdResult<ScanSummary> {
    let state = app.state::<AppState>();
    let engine = state.engine.clone();
    let admin = source.needs_admin();
    let dir = match &workspace {
        Some(folder) => {
            let f = PathBuf::from(folder);
            if !f.is_absolute() || !f.is_dir() {
                return Err(err(Error::Refused(format!(
                    "{folder} is not an existing folder"
                ))));
            }
            workspace::chosen_dir(&f)
        }
        None => workspace::default_dir(&state.data_dir),
    };
    let spec = source.clone();
    let dir = blocking(move || {
        let dir = workspace::approve(&spec, &dir, confirm_unknown.unwrap_or(false))?;
        workspace::sweep(&dir);
        Ok(dir)
    })
    .await?;
    let request = Request::Scan {
        source,
        workspace: Some(dir.to_string_lossy().into_owned()),
    };
    let call = engine.call(request, admin).map_err(err)?;
    state.cancels.lock().insert("scan".into(), call.canceller());
    let handle = app.clone();
    let r = blocking(move || {
        let v = call.wait(|p| emit(&handle, "scan", p), |_| Ok(()))?;
        Ok(serde_json::from_value(v)?)
    })
    .await;
    state.cancels.lock().remove("scan");
    r
}

#[tauri::command]
async fn scan_query(app: AppHandle, scan: u32, query: Query) -> CmdResult<Page> {
    let engine = app.state::<AppState>().engine.clone();
    blocking(move || {
        Ok(serde_json::from_value(call_value(
            &engine,
            Request::Query { scan, query },
            false,
        )?)?)
    })
    .await
}

#[tauri::command]
async fn scan_entries(app: AppHandle, scan: u32, records: Vec<u64>) -> CmdResult<Vec<EntryView>> {
    let engine = app.state::<AppState>().engine.clone();
    blocking(move || {
        Ok(serde_json::from_value(call_value(
            &engine,
            Request::Entries { scan, records },
            false,
        )?)?)
    })
    .await
}

#[tauri::command]
async fn scan_forget(app: AppHandle, scan: u32) -> CmdResult<()> {
    let engine = app.state::<AppState>().engine.clone();
    blocking(move || call_value(&engine, Request::Forget { scan }, false).map(|_| ())).await
}

// ---------------------------------------------------------------- destination guard and recovery

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuardView {
    verdict: Verdict,
    source: DiskSet,
    dest: DiskSet,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum GuardSource {
    Spec {
        spec: SourceSpec,
    },
    /// Every drive that holds one of these Recycle Bin items.
    Bin {
        ids: Vec<String>,
    },
}

#[tauri::command]
async fn guard_check(app: AppHandle, source: GuardSource, dest: String) -> CmdResult<GuardView> {
    let sid = app.state::<AppState>().sid.clone();
    blocking(move || {
        let d = PathBuf::from(&dest);
        if !d.is_absolute() || !d.is_dir() {
            return Err(Error::Refused(format!("{dest} is not an existing folder")));
        }
        let dest = guard::disks_of_path(&d)?;
        let source = match source {
            GuardSource::Spec { spec } => sources::source_disks(&spec)?,
            GuardSource::Bin { ids } => {
                let roots: Vec<String> = disks::drives().into_iter().map(|d| d.root).collect();
                let mut merged = DiskSet::default();
                for item in recycle::list(&sid, &roots)
                    .into_iter()
                    .filter(|i| ids.contains(&i.id))
                {
                    let s = sources::source_disks(&SourceSpec::Volume { root: item.drive })?;
                    merged.disks.extend(s.disks);
                    merged.unknown = merged.unknown.or(s.unknown);
                }
                merged.disks.sort_unstable();
                merged.disks.dedup();
                merged
            }
        };
        Ok(GuardView {
            verdict: guard::verdict(&source, &dest),
            source,
            dest,
        })
    })
    .await
}

#[tauri::command]
async fn recover_start(app: AppHandle, request: recover::RecoverRequest) -> CmdResult<RunSummary> {
    let state = app.state::<AppState>();
    let (engine, sid) = (state.engine.clone(), state.sid.clone());
    let version = app.package_info().version.to_string();
    run_job(&app, "recover", move |job| {
        let ctx = recover::Ctx {
            engine: &engine,
            sid: &sid,
            version: &version,
            job: &job,
        };
        recover::run(&ctx, request)
    })
    .await
}

// ---------------------------------------------------------------- startup

pub fn run() {
    let data_dir = settings::data_dir();
    // Databases a crash left behind; scans with a folder the user picked sweep theirs on use.
    workspace::sweep(&workspace::default_dir(&data_dir));
    let state = AppState {
        engine: Engine::new(workspace::default_dir(&data_dir)),
        settings: RwLock::new(Settings::load(&data_dir)),
        sid: system::current_user_sid().unwrap_or_default(),
        cancels: Mutex::new(HashMap::new()),
        data_dir: data_dir.clone(),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .setup(move |app| {
            // The WebView2 profile goes to the app's own data folder (next to the exe when
            // portable) and runs InPrivate, so browsing data is not kept between runs. A quick
            // restart can find the profile still held by the previous run's WebView2 processes
            // ("resource in use") while they shut down, so creation is retried for up to 20 s.
            let mut attempt = 0;
            loop {
                let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                    .title("Unlost Recovery")
                    .inner_size(1280.0, 840.0)
                    .min_inner_size(960.0, 640.0)
                    .center()
                    .data_directory(data_dir.join("webview"))
                    .incognito(true)
                    .disable_drag_drop_handler()
                    .build();
                match built {
                    Ok(_) => return Ok(()),
                    Err(_) if attempt < 40 => {
                        attempt += 1;
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            settings_get,
            settings_set,
            job_cancel,
            reveal_path,
            drives,
            media_devices,
            admin_start,
            recycle_list,
            shadow_list,
            shadow_dir,
            image_partitions,
            scan_start,
            scan_query,
            scan_entries,
            scan_forget,
            guard_check,
            recover_start,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Unlost")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Session databases are caches of this run only.
                app.state::<AppState>().engine.forget_all();
            }
        });
}

mod commands;
mod db;
mod gmail;
mod logging;
mod models;
mod oauth;

use commands::AppState;
use std::time::Duration;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = db::Db::open(&dir.join("mailhub.db"))?;
            app.manage(AppState::new(db));

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                loop {
                    let state = handle.state::<AppState>();
                    let accounts = state.db.accounts().unwrap_or_default();
                    for acc in accounts {
                        let stale = acc
                            .last_sync
                            .map(|t| db::now_ms() - t > 60_000)
                            .unwrap_or(true);
                        if stale {
                            let _ =
                                commands::sync_account_inner(&handle, state.inner(), &acc.id).await;
                        }
                    }
                    drop(state);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::setup_status,
            commands::set_oauth_client,
            commands::list_accounts,
            commands::add_account,
            commands::submit_auth_url,
            commands::cancel_login,
            commands::login_url,
            commands::remove_account,
            commands::sync_account,
            commands::sync_all,
            commands::list_messages,
            commands::list_labels,
            commands::get_message,
            commands::search_messages,
            commands::set_labels,
            commands::trash_message,
            commands::send_message,
            commands::download_attachment,
            commands::open_path,
            commands::open_log,
            commands::autostart_enable,
            commands::autostart_disable,
            commands::autostart_is_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MailHub");
}

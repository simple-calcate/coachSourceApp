mod exporters;
mod importer;
mod model;
mod settings;
mod store;
mod sync;
mod tokenizer;

use tauri_plugin_opener::OpenerExt;

/// 在系统文件管理器里打开某个路径 / 选中某个文件（仅桌面端）
#[cfg(desktop)]
#[tauri::command]
fn reveal_in_finder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(path)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 手机端没有可调用的文件管理器
#[cfg(mobile)]
#[tauri::command]
fn reveal_in_finder(_app: tauri::AppHandle, _path: String) -> Result<(), String> {
    Err("手机端无法打开文件管理器，请在导出页查看路径".to_string())
}

/// 用系统浏览器打开网址（设备码登录、查看云端文件时用）
#[tauri::command]
async fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 第一次运行时把数据目录建出来。就几个 create_dir_all，毫秒级，
            // 直接同步做——放后台线程反而有"前端抢在目录建好前读写"的竞态。
            store::init_dirs(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            store::get_config,
            store::get_stats,
            store::list_records,
            store::get_record,
            store::save_record,
            store::delete_record,
            store::delete_records,
            store::save_image,
            store::delete_image,
            store::mark_exported,
            store::restore_records,
            store::reset_exported,
            store::archive_records,
            store::restore_from_archive,
            store::purge_records,
            exporters::export_data,
            exporters::mirror,
            importer::import_text,
            importer::import_file,
            importer::pick_import_file,
            importer::pick_export_dir,
            sync::github_get_config,
            sync::github_save_config,
            sync::github_device_start,
            sync::github_device_poll,
            sync::github_finish_login,
            sync::github_logout,
            sync::github_test_connection,
            sync::github_ensure_repo,
            sync::github_set_visibility,
            sync::github_sync,
            settings::get_app_settings,
            settings::save_app_settings,
            tokenizer::tokenizer_analyze,
            tokenizer::tokenizer_vocab,
            tokenizer::tokenizer_add_words,
            tokenizer::tokenizer_remove_words,
            tokenizer::tokenizer_import_trie,
            tokenizer::tokenizer_export_file,
            reveal_in_finder,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("启动应用失败")
}

use std::{sync::Mutex, time::Duration};
use tauri::{Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct UpdateState(pub Mutex<Option<Update>>);

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    current_version: String,
    available_version: Option<String>,
    notes: Option<String>,
}

#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<UpdateStatus, String> {
    let update = app.updater_builder().timeout(Duration::from_secs(30))
        .build().map_err(|error| error.to_string())?
        .check().await.map_err(|error| format!("Update check failed: {error}"))?;
    let status = UpdateStatus {
        current_version: app.package_info().version.to_string(),
        available_version: update.as_ref().map(|update| update.version.clone()),
        notes: update.as_ref().and_then(|update| update.body.clone()),
    };
    *app.state::<UpdateState>().0.lock().map_err(|_| "Update lock failed")? = update;
    Ok(status)
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ReleaseNote {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
}

#[tauri::command]
pub async fn update_log() -> Result<Vec<ReleaseNote>, String> {
    let releases: Vec<ReleaseNote> = reqwest::Client::builder()
        .timeout(Duration::from_secs(30)).user_agent("PERPHEADS-Texture-Helper")
        .build().map_err(|error| error.to_string())?
        .get("https://api.github.com/repos/JoeyMarrey29/TextureHelper/releases?per_page=10")
        .send().await.map_err(|error| format!("Release log request failed: {error}"))?
        .error_for_status().map_err(|error| format!("Release log unavailable: {error}"))?
        .json().await.map_err(|error| format!("Invalid release log: {error}"))?;
    Ok(releases.into_iter().filter(|release| !release.draft && !release.prerelease).collect())
}

#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let update = app.state::<UpdateState>().0.lock().map_err(|_| "Update lock failed")?
        .take().ok_or("Check for updates before installing")?;
    let mut downloaded = 0_u64;
    let result = update.download_and_install(
        |chunk, total| {
            downloaded += chunk as u64;
            if let Err(error) = app.emit("update-progress", (downloaded, total)) {
                eprintln!("Could not report update progress: {error}");
            }
        }, || {},
    ).await;
    if let Err(error) = result {
        *app.state::<UpdateState>().0.lock().map_err(|_| "Update lock failed")? = Some(update);
        return Err(format!("Update installation failed: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Downloads the published GitHub release; never runs the installer"]
    fn published_release_signature_and_version_are_verified() {
        let config: serde_json::Value = serde_json::from_str(include_str!("..\\tauri.conf.json")).unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert("updater".into(), config["plugins"]["updater"].clone());
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context).unwrap();
        tauri::async_runtime::block_on(async {
            let updater = app.handle().updater_builder()
                .target("windows-x86_64")
                .version_comparator(|_, _| true)
                .timeout(Duration::from_secs(60)).build().unwrap();
            let update = updater.check().await.unwrap().expect("Published release missing");
            assert_eq!(update.version, config["version"].as_str().unwrap());
            let bytes = update.download(|_, _| {}, || {}).await.unwrap();
            assert!(bytes.starts_with(b"MZ"));
            update.verify(&bytes).unwrap();
            let mut corrupted = bytes.clone();
            corrupted[100] ^= 1;
            assert!(update.verify(&corrupted).is_err());
            let mut wrong_version = update.clone();
            wrong_version.version = "99.0.0".into();
            assert!(wrong_version.verify(&bytes).is_err());
        });
    }
}

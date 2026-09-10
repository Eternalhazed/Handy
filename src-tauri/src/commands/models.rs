use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::{ModelInfo, ModelManager};
use crate::managers::transcription::{ModelStateEvent, TranscriptionManager};
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout, TranscriptionProvider};
use log::error;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
#[specta::specta]
pub async fn get_available_models(
    model_manager: State<'_, Arc<ModelManager>>,
) -> Result<Vec<ModelInfo>, String> {
    Ok(model_manager.get_available_models())
}

#[tauri::command]
#[specta::specta]
pub async fn get_model_info(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> Result<Option<ModelInfo>, String> {
    Ok(model_manager.get_model_info(&model_id))
}

/// Re-scan local sources (custom models dir + shared HF cache) for models added
/// since launch
#[tauri::command]
#[specta::specta]
pub async fn rescan_local_models(
    model_manager: State<'_, Arc<ModelManager>>,
) -> Result<(), String> {
    let mm = model_manager.inner().clone();
    tokio::task::spawn_blocking(move || mm.rescan_local_models())
        .await
        .map_err(|e| format!("rescan task panicked: {e}"))?
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn download_model(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> Result<(), String> {
    let result = model_manager
        .download_model(&model_id)
        .await
        .map_err(|e| e.to_string());

    if let Err(ref error) = result {
        // Log as well as emit: the toast is transient, and failed downloads have
        // historically been undiagnosable because logs showed nothing (#1579).
        error!("Model download failed for {}: {}", model_id, error);
        let _ = app_handle.emit(
            "model-download-failed",
            serde_json::json!({ "model_id": &model_id, "error": error }),
        );
    }

    result
}

#[tauri::command]
#[specta::specta]
pub async fn delete_model(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    model_id: String,
) -> Result<(), String> {
    // If deleting the active model, unload it and clear the setting
    let settings = get_settings(&app_handle);
    if settings.selected_model == model_id {
        transcription_manager
            .unload_model()
            .map_err(|e| format!("Failed to unload model: {}", e))?;

        let mut settings = get_settings(&app_handle);
        settings.selected_model = String::new();
        write_settings(&app_handle, settings);
    }

    model_manager
        .delete_model(&model_id)
        .map_err(|e| e.to_string())
}

/// Shared logic for switching the active model, used by both the Tauri command
/// and the tray menu handler.
///
/// Validates the model, updates the persisted setting, and loads the model
/// unless the unload timeout is set to "Immediately" (in which case the model
/// will be loaded on-demand during the next transcription).
pub fn switch_active_model(app: &AppHandle, model_id: &str) -> Result<(), String> {
    let model_manager = app.state::<Arc<ModelManager>>();
    let transcription_manager = app.state::<Arc<TranscriptionManager>>();

    // A live dictation is already in flight; switching the model under it would
    // change what the running operation was started for.
    if app.state::<Arc<AudioRecordingManager>>().is_recording() {
        return Err("Transcription is in progress".to_string());
    }

    // Selecting a local model always switches the backend back to local —
    // including when OpenRouter is currently active.
    let Some(_operation_guard) = transcription_manager.try_acquire_operation() else {
        return Err("Transcription is in progress".to_string());
    };

    // Atomically claim the loading slot — prevents concurrent model loads
    // from tray double-clicks or overlapping commands. The guard resets the
    // flag on drop (including early returns, errors, and panics).
    let _loading_guard = transcription_manager
        .try_start_loading()
        .ok_or_else(|| "Model load already in progress".to_string())?;

    // Check if model exists and is available
    let model_info = model_manager
        .get_model_info(model_id)
        .ok_or_else(|| format!("Model not found: {}", model_id))?;

    if !model_info.is_downloaded {
        return Err(format!("Model not downloaded: {}", model_id));
    }

    let settings = get_settings(app);
    let unload_timeout = settings.model_unload_timeout;
    let old_model = settings.selected_model.clone();
    let old_onboarding_completed = settings.onboarding_completed;
    let old_provider = settings.transcription_provider;

    // Persist the new selection early so the frontend sees the correct model
    // when it reacts to events emitted by load_model.
    let mut settings = settings;
    settings.selected_model = model_id.to_string();
    settings.transcription_provider = TranscriptionProvider::Local;
    settings.onboarding_completed = true;

    write_settings(app, settings);

    // Skip eager loading if unload is set to "Immediately" — the model
    // will be loaded on-demand during the next transcription.
    if unload_timeout == ModelUnloadTimeout::Immediately {
        // Notify frontend — load_model won't be called so no events
        // would otherwise be emitted.
        let _ = app.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "selection_changed".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );
        log::info!(
            "Model selection changed to {} (not loading — unload set to Immediately).",
            model_id
        );
        return Ok(());
    }

    // Load the model. On failure, revert the persisted selection.
    if let Err(e) = transcription_manager.load_model(model_id) {
        let mut settings = get_settings(app);
        settings.selected_model = old_model;
        settings.onboarding_completed = old_onboarding_completed;
        settings.transcription_provider = old_provider;
        write_settings(app, settings);
        return Err(e.to_string());
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_active_model(
    app_handle: AppHandle,
    _model_manager: State<'_, Arc<ModelManager>>,
    _transcription_manager: State<'_, Arc<TranscriptionManager>>,
    model_id: String,
) -> Result<(), String> {
    switch_active_model(&app_handle, &model_id)
}

#[tauri::command]
#[specta::specta]
pub async fn get_current_model(app_handle: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app_handle);
    Ok(settings.selected_model)
}

#[tauri::command]
#[specta::specta]
pub async fn get_transcription_model_status(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> Result<Option<String>, String> {
    Ok(transcription_manager.get_current_model())
}

#[tauri::command]
#[specta::specta]
pub async fn is_model_loading(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> Result<bool, String> {
    // Check if transcription manager has a loaded model
    let current_model = transcription_manager.get_current_model();
    Ok(current_model.is_none())
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_download(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> Result<(), String> {
    model_manager
        .cancel_download(&model_id)
        .map_err(|e| e.to_string())
}

/// Every speech-to-text model OpenRouter currently offers (public catalog).
///
/// Nothing is persisted: the catalog is only needed while the OpenRouter
/// settings UI is open, and a stored list would go stale silently.
#[tauri::command]
#[specta::specta]
pub async fn fetch_openrouter_transcription_models() -> Result<Vec<String>, String> {
    crate::openrouter_stt::fetch_models().await
}

/// Switch transcription to OpenRouter with `model_id`, shared by the command and
/// the tray item.
///
/// The id comes from the caller (a discovered catalog entry or the previously
/// saved choice); no catalog is persisted just to validate membership, and no
/// billable request is made to verify the selection.
pub fn apply_openrouter_transcription_model(app: &AppHandle, model_id: &str) -> Result<(), String> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("Select an OpenRouter transcription model first".to_string());
    }

    let settings = get_settings(app);
    let has_key = settings
        .post_process_api_keys
        .get(crate::openrouter_stt::PROVIDER_ID)
        .is_some_and(|key| !key.trim().is_empty());
    if !has_key {
        return Err("Add an OpenRouter API key first".to_string());
    }

    if settings.transcription_provider == TranscriptionProvider::OpenRouter
        && settings.openrouter_transcription_model == model_id
    {
        // Already the active selection; nothing to unload or rewrite.
        return Ok(());
    }

    let transcription_manager = app.state::<Arc<TranscriptionManager>>();
    if app.state::<Arc<AudioRecordingManager>>().is_recording() {
        return Err("Transcription is in progress".to_string());
    }

    let Some(_operation_guard) = transcription_manager.try_acquire_operation() else {
        return Err("Transcription is in progress".to_string());
    };

    // A pending native load must not land after the switch. Claiming the loading
    // slot (and dropping the engine) makes the local ASR state empty before the
    // provider flips, so nothing can resurrect a local engine afterwards.
    let Some(_loading_guard) = transcription_manager.try_start_loading() else {
        return Err("Model load already in progress".to_string());
    };
    transcription_manager
        .unload_model()
        .map_err(|e| e.to_string())?;

    let mut settings = get_settings(app);
    settings.transcription_provider = TranscriptionProvider::OpenRouter;
    settings.openrouter_transcription_model = model_id.to_string();
    settings.onboarding_completed = true;
    write_settings(app, settings);

    let _ = app.emit(
        "model-state-changed",
        ModelStateEvent {
            event_type: "selection_changed".to_string(),
            model_id: Some(model_id.to_string()),
            model_name: Some(model_id.to_string()),
            error: None,
        },
    );
    log::info!("Transcription switched to OpenRouter model '{}'", model_id);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn select_openrouter_transcription_model(
    app: AppHandle,
    model_id: String,
) -> Result<(), String> {
    apply_openrouter_transcription_model(&app, &model_id)
}

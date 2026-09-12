//! Fixed-endpoint OpenRouter speech-to-text client.
//!
//! This is the only cloud transcription path in Handy: the model catalog comes
//! from OpenRouter's public `?output_modalities=transcription` listing and audio
//! is uploaded to `/audio/transcriptions`. Both URLs are compile-time constants
//! — there is deliberately no configurable base URL and no environment override,
//! so an authenticated upload can never be aimed at another origin.
//!
//! The API key is the one shared with post-processing
//! ([`crate::settings::AppSettings::post_process_api_keys`]); the transcription
//! model is stored separately so neither pipeline can change the other's choice.

use crate::settings::{AppSettings, PostProcessProvider};
use log::{debug, info, warn};
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

/// Provider id owning the shared OpenRouter API key and attribution headers.
pub(crate) const PROVIDER_ID: &str = "openrouter";

/// OpenRouter API root. Kept only for provider metadata; requests use the
/// explicit endpoint constants below.
const PROVIDER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Public model catalog filtered to speech-to-text models (no API key required).
const MODELS_URL: &str = "https://openrouter.ai/api/v1/models?output_modalities=transcription";
/// Audio transcription endpoint.
const TRANSCRIPTIONS_URL: &str = "https://openrouter.ai/api/v1/audio/transcriptions";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
/// Total budget for one transcription, covering every retry attempt.
const TRANSCRIPTION_BUDGET: Duration = Duration::from_secs(90);
const MAX_TRANSCRIPTION_ATTEMPTS: usize = 3;
/// Backoff before attempt 2 and attempt 3, clamped to the remaining budget.
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(400), Duration::from_millis(1200)];

/// A failed upload, classified by the bounded retry policy.
struct UploadError {
    message: String,
    /// Only connection-establishment failures and HTTP 429 are eligible. This
    /// relies on the provider's rate-limit semantics, not an idempotency or
    /// billing guarantee. Ambiguous transport failures and every 5xx are final.
    retryable: bool,
}

impl UploadError {
    fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

/// Shared client: connection pooling, no default headers (the key is attached
/// per request so a client can never carry a credential the caller did not pass)
/// and no redirect following (an authenticated upload must not be replayed to
/// another origin).
static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| {
            crate::llm_client::report_reqwest_error(
                "Failed to build the OpenRouter HTTP client",
                &e,
            )
        })
});

fn client() -> Result<&'static reqwest::Client, String> {
    CLIENT.as_ref().map_err(Clone::clone)
}

/// Provider metadata used only to derive the shared attribution/auth headers.
fn provider() -> PostProcessProvider {
    PostProcessProvider {
        id: PROVIDER_ID.to_string(),
        label: "OpenRouter".to_string(),
        base_url: PROVIDER_BASE_URL.to_string(),
        allow_base_url_edit: false,
        models_endpoint: Some("/models".to_string()),
        supports_structured_output: true,
    }
}

/// The language hint to send for the current settings, or `None` for
/// auto-detect. Chinese script variants are not ISO-639-1 codes, so they are
/// reduced to `zh`; the script intent is applied to the output downstream.
pub(crate) fn language_hint(settings: &AppSettings) -> Option<String> {
    let language = settings.selected_language.trim();
    if language.is_empty() || language.eq_ignore_ascii_case("auto") {
        return None;
    }
    Some(crate::managers::model::recognition_language(language).to_string())
}

/// Fetch every speech-to-text model OpenRouter currently offers.
///
/// The listing is public, so no API key is sent. A malformed envelope is an
/// error; a well-formed empty list is a valid answer (no STT models offered).
pub(crate) async fn fetch_models() -> Result<Vec<String>, String> {
    fetch_models_from(MODELS_URL, DISCOVERY_TIMEOUT).await
}

async fn fetch_models_from(url: &str, timeout: Duration) -> Result<Vec<String>, String> {
    let client = client()?;
    let headers = crate::llm_client::build_headers(&provider(), "")?;
    let deadline = tokio::time::Instant::now() + timeout;

    // Discovery is not retried: the ui has an explicit Refresh action, and a
    // stale list is never worse than a delayed one.
    let body = exchange(
        "Failed to fetch the OpenRouter model list",
        "model list",
        client.get(url).headers(headers),
        deadline,
    )
    .await
    .map_err(|error| error.message)?;

    let parsed: Value = serde_json::from_slice(&body)
        .map_err(|_| "OpenRouter returned a malformed model list".to_string())?;
    parse_models(&parsed)
}

/// Run one HTTP exchange — send, status check and body read — under a shared
/// deadline.
///
/// The deadline covers the whole exchange, so a slow response cannot take twice
/// the configured time. Each failure is classified under the retry policy.
async fn exchange(
    context: &str,
    label: &str,
    request: reqwest::RequestBuilder,
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>, UploadError> {
    let response = match tokio::time::timeout_at(deadline, request.send()).await {
        Ok(Ok(response)) => response,
        // Only a failure to *establish* the connection proves the provider never
        // received the request. Any other transport failure — the connection was
        // lost after the body went out, or response headers never arrived — may
        // have been processed and billed, so it is reported as final. History
        // offers a manual retry for exactly that case.
        Ok(Err(error)) => {
            let message = crate::llm_client::report_reqwest_error(context, &error);
            return Err(if error.is_connect() {
                UploadError::retryable(message)
            } else {
                UploadError::fatal(message)
            });
        }
        // The deadline elapsed with the request already in flight; the provider
        // may have transcribed it, so this must not be replayed.
        Err(_) => {
            return Err(UploadError::fatal(format!("{} ({})", TIMED_OUT, label)));
        }
    };

    let status = response.status();
    if !status.is_success() {
        return Err(status_error(label, status));
    }

    // Past this point the provider has accepted the upload (it returned a
    // status), so a failed or truncated body must not be retried: the request
    // may already have been transcribed and billed.
    match tokio::time::timeout_at(deadline, response.bytes()).await {
        Ok(Ok(bytes)) => Ok(bytes.to_vec()),
        Ok(Err(error)) => Err(UploadError::fatal(crate::llm_client::report_reqwest_error(
            "Failed to read the OpenRouter response",
            &error,
        ))),
        Err(_) => Err(UploadError::fatal(format!(
            "{} ({} response)",
            TIMED_OUT, label
        ))),
    }
}

/// Keep only entries that are speech-to-text capable: audio in, transcription
/// out. Chat models that merely mention audio are not transcription models.
fn parse_models(parsed: &Value) -> Result<Vec<String>, String> {
    let data = parsed
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "OpenRouter model list response has no data array".to_string())?;

    let mut ids: Vec<String> = Vec::new();
    for entry in data {
        if !lists_modality(entry, "input_modalities", "audio")
            || !lists_modality(entry, "output_modalities", "transcription")
        {
            continue;
        }
        if let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            ids.push(id.to_string());
        }
    }

    ids.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    ids.dedup();
    Ok(ids)
}

fn lists_modality(entry: &Value, key: &str, modality: &str) -> bool {
    entry
        .get("architecture")
        .and_then(|architecture| architecture.get(key))
        .and_then(Value::as_array)
        .is_some_and(|modalities| {
            modalities
                .iter()
                .any(|value| value.as_str() == Some(modality))
        })
}

/// Transcribe recorded samples through OpenRouter.
///
/// `samples` are 16 kHz mono f32 (the app's recording format). Empty audio
/// returns empty text without contacting the network, matching the local
/// engines' "no speech" handling.
pub(crate) async fn transcribe(
    samples: Vec<f32>,
    settings: &AppSettings,
) -> anyhow::Result<String> {
    transcribe_to(TRANSCRIPTIONS_URL, samples, settings).await
}

/// [`transcribe`] against an explicit endpoint. Production always passes the
/// fixed constant; tests use a loopback address so the upload path and its retry
/// behaviour are exercised for real (there is no runtime override).
async fn transcribe_to(
    endpoint: &str,
    samples: Vec<f32>,
    settings: &AppSettings,
) -> anyhow::Result<String> {
    transcribe_with_budget(endpoint, samples, settings, TRANSCRIPTION_BUDGET).await
}

/// Upload the recording with bounded retries inside one total network budget.
///
/// A retry may re-upload audio. Only connection-establishment failures and HTTP
/// 429 rate-limit rejections are retried; this is not an exactly-once or billing
/// guarantee. Any ambiguous result (a dropped connection after sending, any 5xx
/// including 503, or a missing/unusable response) is final because the audio may
/// already have been processed. HTTP 503 does not establish otherwise for STT.
/// When the retries are exhausted the error surfaces as usual, the recording is
/// preserved, and History offers a manual retry.
async fn transcribe_with_budget(
    endpoint: &str,
    samples: Vec<f32>,
    settings: &AppSettings,
    budget: Duration,
) -> anyhow::Result<String> {
    let model = settings.openrouter_transcription_model.trim().to_string();
    if model.is_empty() {
        anyhow::bail!("No OpenRouter transcription model is selected");
    }

    let api_key = settings
        .post_process_api_keys
        .get(PROVIDER_ID)
        .cloned()
        .unwrap_or_default();
    if api_key.trim().is_empty() {
        anyhow::bail!("OpenRouter transcription needs an API key");
    }

    if samples.is_empty() {
        return Ok(String::new());
    }

    let language = language_hint(settings);
    let sample_count = samples.len();

    // Encoding, base64 and the JSON body are pure CPU work; keep them off the
    // async worker so a long recording cannot stall other tasks.
    let payload =
        tauri::async_runtime::spawn_blocking(move || build_payload(samples, model, language))
            .await
            .map_err(|e| anyhow::anyhow!("Audio encoding task failed: {e}"))?
            .map_err(anyhow::Error::msg)?;

    debug!(
        "Uploading {} samples ({} bytes encoded) to OpenRouter for transcription",
        sample_count,
        payload.len()
    );

    let deadline = tokio::time::Instant::now() + budget;
    let mut attempt = 1;
    loop {
        match transcribe_attempt(endpoint, &api_key, payload.clone(), deadline).await {
            Ok(text) => {
                if attempt > 1 {
                    info!("OpenRouter transcription succeeded on attempt {}", attempt);
                }
                return Ok(text);
            }
            Err(error)
                if error.retryable
                    && attempt < MAX_TRANSCRIPTION_ATTEMPTS
                    && tokio::time::Instant::now() < deadline =>
            {
                let wait = RETRY_BACKOFF[attempt - 1]
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                warn!(
                    "OpenRouter transcription attempt {} of {} failed ({}); retrying in {:?}",
                    attempt, MAX_TRANSCRIPTION_ATTEMPTS, error.message, wait
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(error) => return Err(anyhow::anyhow!(error.message)),
        }
    }
}

/// One upload attempt; the retry policy lives in [`transcribe_with_budget`].
async fn transcribe_attempt(
    url: &str,
    api_key: &str,
    payload: Vec<u8>,
    deadline: tokio::time::Instant,
) -> Result<String, UploadError> {
    let client = client().map_err(UploadError::fatal)?;
    let headers =
        crate::llm_client::build_headers(&provider(), api_key).map_err(UploadError::fatal)?;

    let body = exchange(
        "OpenRouter transcription request failed",
        "transcription",
        client.post(url).headers(headers).body(payload),
        deadline,
    )
    .await?;

    let parsed: Value = serde_json::from_slice(&body).map_err(|_| {
        // Processing may have completed, so this unusable response is final.
        UploadError::fatal("OpenRouter returned a malformed transcription response")
    })?;

    parsed
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| UploadError::fatal("OpenRouter transcription response contained no text"))
}

fn build_payload(
    samples: Vec<f32>,
    model: String,
    language: Option<String>,
) -> Result<Vec<u8>, String> {
    use base64::Engine as _;

    let wav = crate::audio_toolkit::encode_wav(&samples)
        .map_err(|e| format!("Failed to encode the recording as WAV: {e}"))?;

    let mut body = json!({
        "model": model,
        "input_audio": {
            "data": base64::engine::general_purpose::STANDARD.encode(&wav),
            "format": "wav",
        },
        "response_format": "json",
    });
    if let Some(language) = language {
        body["language"] = Value::String(language);
    }

    serde_json::to_vec(&body).map_err(|e| format!("Failed to serialize the request: {e}"))
}

/// Single-attempt transport, used by tests to observe one exchange at a time.
/// Production goes through [`transcribe_attempt`] via the retry loop.
#[cfg(test)]
async fn transcribe_request(
    url: &str,
    api_key: &str,
    payload: Vec<u8>,
    timeout: Duration,
) -> Result<String, String> {
    transcribe_attempt(url, api_key, payload, tokio::time::Instant::now() + timeout)
        .await
        .map_err(|error| error.message)
}

const TIMED_OUT: &str = "OpenRouter request timed out";

/// Map a non-2xx status to an actionable message. The response body is never
/// included: it can echo request data and would leak into logs and toasts.
///
/// HTTP 429 is eligible under the rate-limit retry policy. No 5xx, including
/// 503, establishes that an STT upload was not processed; replaying it could
/// upload and bill the same audio twice.
fn status_error(context: &str, status: reqwest::StatusCode) -> UploadError {
    let message = match status.as_u16() {
        401 => {
            "OpenRouter rejected the API key (401). Re-enter it in Settings → Models.".to_string()
        }
        402 => "OpenRouter has insufficient credits for this request (402).".to_string(),
        403 => "OpenRouter denied access to this model (403).".to_string(),
        404 => "This transcription model is unavailable on OpenRouter (404). Pick another model."
            .to_string(),
        413 => "The recording is too large for OpenRouter (413).".to_string(),
        429 => "OpenRouter rate-limited the request (429). Try again in a moment.".to_string(),
        _ => format!("{context} failed with status {status}"),
    };

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        UploadError::retryable(message)
    } else {
        UploadError::fatal(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::transcription::{
        post_process_transcription_text, resolve_output_language_evidence,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn settings_for(provider: crate::settings::TranscriptionProvider) -> AppSettings {
        AppSettings {
            transcription_provider: provider,
            openrouter_transcription_model: "openai/whisper-large-v3".to_string(),
            ..Default::default()
        }
    }

    fn settings_with_key(key: &str) -> AppSettings {
        let mut settings = settings_for(crate::settings::TranscriptionProvider::OpenRouter);
        settings
            .post_process_api_keys
            .insert(PROVIDER_ID.to_string(), key.to_string());
        settings
    }

    fn catalog(entries: Value) -> Value {
        json!({ "data": entries })
    }

    fn stt_entry(id: &str) -> Value {
        json!({
            "id": id,
            "architecture": {
                "input_modalities": ["audio"],
                "output_modalities": ["transcription"]
            }
        })
    }

    #[test]
    fn openrouter_stt_catalog_keeps_only_transcription_models() {
        let parsed = catalog(json!([
            stt_entry("openai/whisper-large-v3"),
            // Chat model that accepts audio but answers with text: not STT.
            json!({
                "id": "some/audio-chat",
                "architecture": {
                    "input_modalities": ["audio", "text"],
                    "output_modalities": ["text"]
                }
            }),
            // Transcription-capable list endpoint without audio input.
            json!({
                "id": "some/text-to-text",
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["transcription"]
                }
            }),
            // No architecture block at all.
            json!({ "id": "some/unknown" }),
            json!({ "id": "   " }),
            json!({ "name": "missing id" }),
            stt_entry("nvidia/parakeet-tdt-0.6b-v3"),
        ]));

        let ids = parse_models(&parsed).unwrap();
        assert_eq!(
            ids,
            vec![
                "nvidia/parakeet-tdt-0.6b-v3".to_string(),
                "openai/whisper-large-v3".to_string()
            ]
        );
    }

    #[test]
    fn openrouter_stt_catalog_deduplicates_and_requires_data_array() {
        let duplicated = catalog(json!([
            stt_entry("b/second"),
            stt_entry("a/first"),
            stt_entry("b/second"),
        ]));
        assert_eq!(
            parse_models(&duplicated).unwrap(),
            vec!["a/first".to_string(), "b/second".to_string()]
        );

        assert!(parse_models(&json!({ "error": "nope" })).is_err());
        assert!(parse_models(&json!([stt_entry("a/first")])).is_err());
    }

    #[test]
    fn openrouter_stt_language_hint_maps_script_variants() {
        let hint = |selected: &str| {
            let settings = AppSettings {
                selected_language: selected.to_string(),
                ..Default::default()
            };
            language_hint(&settings)
        };

        assert_eq!(hint("auto"), None);
        assert_eq!(hint("  Auto "), None);
        assert_eq!(hint("zh-Hans"), Some("zh".to_string()));
        assert_eq!(hint("zh-Hant"), Some("zh".to_string()));
        assert_eq!(hint("pt-BR"), Some("pt-BR".to_string()));
    }

    #[tokio::test]
    async fn openrouter_stt_empty_audio_is_not_uploaded() {
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits_for_server = Arc::clone(&hits);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                hits_for_server.fetch_add(1, Ordering::SeqCst);
                let _ = stream.shutdown().await;
            }
        });

        // The guard lives on the real upload path, so the loopback server is what
        // would see empty audio if it were ever sent.
        let settings = settings_with_key("test-key");
        let text = transcribe_to(
            &format!("http://{address}/audio/transcriptions"),
            Vec::new(),
            &settings,
        )
        .await
        .unwrap();

        assert!(text.is_empty());
        assert_eq!(hits.load(Ordering::SeqCst), 0, "empty audio sent a request");
    }

    #[tokio::test]
    async fn openrouter_stt_upload_sends_pcm16_mono_16k_wav_and_returns_text() {
        let (base_url, received) =
            serve_once("200 OK", r#"{"text":"OpenRouter transcription test."}"#).await;
        let endpoint = format!("{base_url}/audio/transcriptions");

        let samples: Vec<f32> = (0..1600)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();

        let payload = build_payload(
            samples.clone(),
            "openai/whisper-large-v3".to_string(),
            Some("en".to_string()),
        )
        .unwrap();
        let text = transcribe_request(&endpoint, "test-key", payload, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(text, "OpenRouter transcription test.");

        let request = received.await.unwrap();
        let (head, body) = split_request(&request);
        let head = String::from_utf8_lossy(head);
        assert!(head.starts_with("POST /audio/transcriptions "));
        assert!(
            head.to_lowercase()
                .contains("authorization: bearer test-key"),
            "the API key was not attached: {head}"
        );

        let json: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(json["model"], "openai/whisper-large-v3");
        assert_eq!(json["response_format"], "json");
        assert_eq!(json["language"], "en");
        assert_eq!(json["input_audio"]["format"], "wav");

        use base64::Engine as _;
        let wav = base64::engine::general_purpose::STANDARD
            .decode(json["input_audio"]["data"].as_str().unwrap())
            .unwrap();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let decoded: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(decoded.len(), samples.len());
        assert_eq!(decoded[0], (0.5 * i16::MAX as f32) as i16);
        assert_eq!(decoded[1], (-0.5 * i16::MAX as f32) as i16);
    }

    #[tokio::test]
    async fn openrouter_stt_error_status_is_reported_without_response_body() {
        let (base_url, _received) = serve_once(
            "401 Unauthorized",
            r#"{"error":"PRIVATE PROVIDER DETAIL sk-or-secret"}"#,
        )
        .await;

        let error = transcribe_request(
            &format!("{base_url}/audio/transcriptions"),
            "test-key",
            br#"{"model":"m"}"#.to_vec(),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();

        assert!(error.contains("401"), "unexpected error: {error}");
        assert!(!error.contains("PRIVATE PROVIDER DETAIL"));
        assert!(!error.contains("sk-or-secret"));
    }

    #[tokio::test]
    async fn openrouter_stt_malformed_response_is_rejected() {
        let (base_url, _received) = serve_once("200 OK", r#"{"choices":[]}"#).await;

        let error = transcribe_request(
            &format!("{base_url}/audio/transcriptions"),
            "test-key",
            br#"{"model":"m"}"#.to_vec(),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();

        assert!(
            error.contains("no text"),
            "expected a missing-text error, got: {error}"
        );
    }

    #[tokio::test]
    async fn openrouter_stt_stalled_upload_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // Accept the connection, read the request, then never answer.
        let stall = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).await;
            std::future::pending::<()>().await;
        });

        let error = transcribe_request(
            &format!("http://{address}/audio/transcriptions"),
            "test-key",
            br#"{"model":"m"}"#.to_vec(),
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();

        assert!(error.contains("timed out"), "unexpected error: {error}");
        stall.abort();
    }

    #[tokio::test]
    async fn openrouter_stt_model_list_parses_public_catalog() {
        let (base_url, _received) = serve_once(
            "200 OK",
            r#"{"data":[
                {"id":"openai/whisper-large-v3","architecture":{"input_modalities":["audio"],"output_modalities":["transcription"]}},
                {"id":"acme/chat","architecture":{"input_modalities":["audio","text"],"output_modalities":["text"]}}
            ]}"#,
        )
        .await;

        let ids = fetch_models_from(&base_url, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(ids, vec!["openai/whisper-large-v3".to_string()]);
    }

    #[tokio::test]
    async fn openrouter_stt_upload_is_cleaned_up_like_local_output() {
        let (base_url, received) = serve_once(
            "200 OK",
            r#"{"text":"  um openrouter transcription test.  "}"#,
        )
        .await;

        let mut settings = settings_with_key("test-key");
        // Custom words and filler removal are configured, so the finished
        // transcript must come out with the same cleanup a local model's output
        // gets — this is the exact sequence the transcription dispatcher runs.
        settings.custom_words = vec!["OpenRouter".to_string()];
        settings.filler_word_removal_enabled = true;
        settings.selected_language = "en".to_string();

        let payload = build_payload(
            vec![0.25_f32; 800],
            settings.openrouter_transcription_model.clone(),
            language_hint(&settings),
        )
        .unwrap();
        let raw = transcribe_request(
            &format!("{base_url}/audio/transcriptions"),
            "test-key",
            payload,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        let hint = language_hint(&settings);
        let evidence = resolve_output_language_evidence(&settings, hint.as_deref(), &[], false);
        let cleaned = post_process_transcription_text(raw, &settings, false, &evidence, &[]);

        // Filler removed, custom word restored to its configured spelling,
        // surrounding whitespace normalized.
        assert_eq!(cleaned, "OpenRouter transcription test.");
        let request = received.await.unwrap();
        assert!(split_request(&request).1.starts_with(b"{"));
    }

    #[tokio::test]
    async fn openrouter_stt_cancelled_upload_produces_no_late_result() {
        // A stalled upload that is abandoned (what cancelling does: the app drops
        // the in-flight future) must not yield text afterwards, and must leave
        // the client usable for the next request.
        let stalled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stalled_address = stalled.local_addr().unwrap();
        let stall = tokio::spawn(async move {
            let (mut stream, _) = stalled.accept().await.unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).await;
            std::future::pending::<()>().await;
        });

        let cancelled = tokio::time::timeout(
            Duration::from_millis(150),
            transcribe_request(
                &format!("http://{stalled_address}/audio/transcriptions"),
                "test-key",
                build_payload(vec![0.1_f32; 160], "m".to_string(), None).unwrap(),
                Duration::from_secs(30),
            ),
        )
        .await;
        assert!(cancelled.is_err(), "the stalled upload should not complete");

        // The next request still works: no cancelled state is carried over.
        let (base_url, _received) = serve_once("200 OK", r#"{"text":"second"}"#).await;
        let text = transcribe_request(
            &format!("{base_url}/audio/transcriptions"),
            "test-key",
            build_payload(vec![0.1_f32; 160], "m".to_string(), None).unwrap(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(text, "second");
        stall.abort();
    }

    #[tokio::test]
    async fn openrouter_stt_retries_rate_limit_then_succeeds() {
        let (endpoint, hits) = serve_sequence(vec![
            ("429 Too Many Requests", r#"{"error":"rate limited"}"#),
            ("200 OK", r#"{"text":"recovered"}"#),
        ])
        .await;

        let settings = settings_with_key("test-key");
        let text = transcribe_with_budget(
            &endpoint,
            vec![0.1_f32; 800],
            &settings,
            Duration::from_secs(10),
        )
        .await
        .unwrap();

        assert_eq!(text, "recovered");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "expected one retry");
    }

    #[tokio::test]
    async fn openrouter_stt_service_unavailable_is_not_replayed() {
        // Even with a successful next response available, a 503 cannot tell us
        // whether the first upload was processed and must be reported as final.
        let (endpoint, hits) = serve_sequence(vec![
            (
                "503 Service Unavailable",
                r#"{"error":"PRIVATE PROVIDER DETAIL"}"#,
            ),
            ("200 OK", r#"{"text":"duplicate"}"#),
        ])
        .await;

        let error = transcribe_with_budget(
            &endpoint,
            vec![0.1_f32; 800],
            &settings_with_key("test-key"),
            Duration::from_secs(3),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("503"), "got: {error}");
        assert!(!error.to_string().contains("PRIVATE PROVIDER DETAIL"));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "ambiguous upload was replayed"
        );
    }

    #[tokio::test]
    async fn openrouter_stt_client_errors_are_not_retried() {
        // A rejected upload (bad key) must never be repeated: it would upload the
        // recording again for the same outcome.
        let (endpoint, hits) =
            serve_sequence(vec![("401 Unauthorized", r#"{"error":"invalid key"}"#)]).await;

        let settings = settings_with_key("test-key");
        let error = transcribe_with_budget(
            &endpoint,
            vec![0.1_f32; 800],
            &settings,
            Duration::from_secs(3),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("401"), "got: {error}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "upload was repeated");
    }

    #[tokio::test]
    async fn openrouter_stt_dropped_response_is_not_replayed() {
        // The provider received the upload but the response never arrived (the
        // connection was closed afterwards). The audio may already have been
        // transcribed and billed, so the request must not be sent again.
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits_for_server = Arc::clone(&hits);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                hits_for_server.fetch_add(1, Ordering::SeqCst);
                // Drain the whole request (headers + body) before closing without
                // answering, so the client's failure is "no response" rather than
                // "the request could not be sent".
                let mut buffer = [0_u8; 4096];
                while let Ok(Ok(read)) =
                    tokio::time::timeout(Duration::from_millis(50), stream.read(&mut buffer)).await
                {
                    if read == 0 {
                        break;
                    }
                }
                let _ = stream.shutdown().await;
            }
        });

        let settings = settings_with_key("test-key");
        let error = transcribe_with_budget(
            &format!("http://{address}/audio/transcriptions"),
            vec![0.1_f32; 800],
            &settings,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();

        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a possibly-billed upload was replayed: {error}"
        );
    }

    #[tokio::test]
    async fn openrouter_stt_malformed_success_is_not_retried() {
        // The provider may already have transcribed and billed this upload, so
        // an unusable response is surfaced instead of being sent again.
        let (endpoint, hits) = serve_sequence(vec![("200 OK", r#"{"choices":[]}"#)]).await;

        let settings = settings_with_key("test-key");
        let error = transcribe_with_budget(
            &endpoint,
            vec![0.1_f32; 800],
            &settings,
            Duration::from_secs(3),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("no text"), "got: {error}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "upload was repeated");
    }

    /// Serve `responses` in order (one request each), counting the requests that
    /// actually arrived.
    async fn serve_sequence(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);

        tokio::spawn(async move {
            for (status, body) in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = read_request(&mut stream).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });

        (format!("http://{address}/audio/transcriptions"), hits)
    }

    /// Accept one HTTP request, capture its raw bytes, and answer with `body`.
    /// Returns the base URL and a receiver for the captured request.
    async fn serve_once(
        status_line: &str,
        body: &str,
    ) -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let _ = tx.send(request);
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        });

        (format!("http://{address}"), rx)
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut body_start = None;

        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => raw.extend_from_slice(&buffer[..read]),
            }
            if body_start.is_none() {
                body_start = find(&raw, b"\r\n\r\n").map(|position| position + 4);
            }
            if let Some(start) = body_start {
                let length = content_length(&raw[..start]).unwrap_or(0);
                if raw.len() >= start + length {
                    break;
                }
            }
        }

        raw
    }

    fn split_request(raw: &[u8]) -> (&[u8], &[u8]) {
        match find(raw, b"\r\n\r\n") {
            Some(position) => (&raw[..position], &raw[position + 4..]),
            None => (raw, &[]),
        }
    }

    fn content_length(head: &[u8]) -> Option<usize> {
        let head = String::from_utf8_lossy(head).to_lowercase();
        head.lines()
            .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}

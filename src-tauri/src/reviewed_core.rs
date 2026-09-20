//! Opt-in, checksum-pinned core releases. App and plugin updates stay independent.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

const PROVENANCE_FILE: &str = "reviewed-core.json";
const APP_UPDATE_BLOCK: &str = "Reviewed core channel is active. Official app updates may remove local compatibility patches. Use a trusted signed reviewed app, or switch back after those patches are upstream.";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewedCoreManifest {
    schema_version: u32,
    repository: String,
    tag: String,
    commit: String,
    assets: Vec<ReviewedCoreAsset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedCoreAsset {
    os: String,
    arch: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewedCoreProvenance {
    manifest_url: String,
    repository: String,
    tag: String,
    commit: String,
    archive_sha256: String,
    binary_sha256: String,
    installed_at_unix: u64,
    backup_dir: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewedCoreSettings {
    manifest_url: String,
    installed: Option<ReviewedCoreProvenance>,
}

fn validate_https_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid reviewed release URL".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Reviewed release URLs must use HTTPS without credentials, ports, queries or fragments"
                .into(),
        );
    }
    Ok(url)
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn validate_manifest(manifest: &ReviewedCoreManifest) -> Result<(), String> {
    let repository_parts = manifest.repository.split('/').collect::<Vec<_>>();
    if manifest.schema_version != 1
        || repository_parts.len() != 2
        || repository_parts.iter().any(|p| {
            p.is_empty()
                || *p == "."
                || *p == ".."
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
        || !is_hex(&manifest.commit, 40)
    {
        return Err("Invalid reviewed manifest schema, repository or commit".into());
    }
    if !manifest.tag.starts_with('v') || semver::Version::parse(&manifest.tag[1..]).is_err() {
        return Err("Reviewed release tag must be v followed by a semantic version".into());
    }
    if manifest.assets.is_empty() || manifest.assets.len() > 12 {
        return Err("Reviewed manifest must contain platform assets".into());
    }
    let mut platforms = BTreeSet::new();
    for asset in &manifest.assets {
        if !["darwin", "linux", "windows"].contains(&asset.os.as_str())
            || !["arm64", "amd64"].contains(&asset.arch.as_str())
            || !platforms.insert((&asset.os, &asset.arch))
            || !is_hex(&asset.sha256, 64)
            || asset.size == 0
            || asset.size > 512 * 1024 * 1024
        {
            return Err("Invalid reviewed asset platform, SHA-256 or size".into());
        }
        let url = validate_https_url(&asset.url)?;
        let prefix = format!(
            "/{}/releases/download/{}/",
            manifest.repository, manifest.tag
        );
        let filename = url.path().strip_prefix(&prefix).unwrap_or_default();
        let suffix = if asset.os == "windows" {
            ".zip"
        } else {
            ".tar.gz"
        };
        if url.host_str() != Some("github.com")
            || filename.is_empty()
            || !filename.ends_with(suffix)
            || !filename
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        {
            return Err(
                "Reviewed asset must belong to the declared GitHub repository and release tag"
                    .into(),
            );
        }
    }
    Ok(())
}

fn selected_asset(manifest: &ReviewedCoreManifest) -> Result<&ReviewedCoreAsset, String> {
    let platform = current_core_platform()?;
    let arch = if platform.asset_arch == "aarch64" {
        "arm64"
    } else {
        &platform.asset_arch
    };
    manifest
        .assets
        .iter()
        .find(|asset| asset.os == platform.asset_os && asset.arch == arch)
        .ok_or_else(|| "Reviewed release has no asset for this platform".into())
}

async fn fetch_manifest(config: &GuiConfigFile) -> Result<ReviewedCoreManifest, String> {
    validate_https_url(&config.reviewed_core_manifest_url)?;
    // No mirrors or official-source fallback. Redirects must remain HTTPS.
    let client = reviewed_http_client(config)?;
    let mut response = client
        .get(&config.reviewed_core_manifest_url)
        .send()
        .await
        .map_err(|_| "Could not fetch reviewed core manifest".to_string())?
        .error_for_status()
        .map_err(|e| {
            format!(
                "Reviewed manifest HTTP status: {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read reviewed manifest".to_string())?
    {
        if bytes.len() + chunk.len() > 512 * 1024 {
            return Err("Reviewed manifest exceeds 512 KiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let manifest = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid reviewed core manifest JSON".to_string())?;
    validate_manifest(&manifest)?;
    selected_asset(&manifest)?;
    Ok(manifest)
}

fn reviewed_http_client(config: &GuiConfigFile) -> Result<reqwest::Client, String> {
    build_http_client_with_proxy(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 || attempt.url().scheme() != "https" {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300)),
        &config.proxy_url,
        "Create reviewed update client",
    )
}

pub(crate) fn reviewed_app_update_guard(config: &GuiConfigFile) -> Result<(), String> {
    if config.reviewed_core_manifest_url.is_empty() {
        Ok(())
    } else {
        Err(APP_UPDATE_BLOCK.into())
    }
}

pub(crate) fn reviewed_official_core_guard(config: &GuiConfigFile) -> Result<(), String> {
    if config.reviewed_core_manifest_url.is_empty() {
        Ok(())
    } else {
        Err("Reviewed core channel is active. Use its verified release, or explicitly switch back to the official channel.".into())
    }
}

fn settings(config: &GuiConfigFile) -> Result<ReviewedCoreSettings, String> {
    let install = core_install_dir()?;
    let installed = fs::read(install.join(PROVENANCE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ReviewedCoreProvenance>(&bytes).ok())
        .filter(|record| {
            find_core_binary(&install)
                .and_then(|path| sha256_file(&path).ok())
                .as_deref()
                == Some(record.binary_sha256.as_str())
        });
    Ok(ReviewedCoreSettings {
        manifest_url: config.reviewed_core_manifest_url.clone(),
        installed,
    })
}

#[tauri::command]
pub(crate) fn get_reviewed_core_settings(
    state: tauri::State<'_, GuiConfigState>,
) -> Result<ReviewedCoreSettings, String> {
    settings(&state.snapshot()?)
}

#[tauri::command]
pub(crate) fn set_reviewed_core_manifest(
    app: tauri::AppHandle,
    url: String,
) -> Result<ReviewedCoreSettings, String> {
    let _guard = lock_core_operation(app.state::<CoreProcessState>().inner())?;
    if app.state::<AppUpdateState>().snapshot().running {
        return Err("Wait for the app update to finish".into());
    }
    let url = url.trim().to_string();
    if !url.is_empty() {
        validate_https_url(&url)?;
    }
    let config = app.state::<GuiConfigState>().update(|config| {
        config.reviewed_core_manifest_url = url;
        Ok(())
    })?;
    settings(&config)
}

pub(crate) async fn check_reviewed_core(config: &GuiConfigFile) -> Result<CoreLatest, String> {
    let manifest = fetch_manifest(config).await?;
    let asset = selected_asset(&manifest)?;
    Ok(CoreLatest {
        reviewed: true,
        version: normalize_version(&manifest.tag),
        asset_name: asset.url.rsplit('/').next().unwrap().to_string(),
    })
}

#[derive(Debug, PartialEq)]
struct HealthSnapshot {
    models: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    plugins: BTreeMap<String, serde_json::Value>,
}

fn parse_health(
    models: &serde_json::Value,
    plugins: &serde_json::Value,
) -> Result<HealthSnapshot, String> {
    let list = models
        .get("models")
        .or_else(|| models.get("data"))
        .and_then(serde_json::Value::as_array)
        .ok_or("Core health check requires a model catalog")?;
    let mut model_map = BTreeMap::new();
    for model in list {
        let id = model
            .get("slug")
            .or_else(|| model.get("id"))
            .and_then(serde_json::Value::as_str)
            .ok_or("Model catalog entry has no ID")?;
        let mut fields = BTreeMap::new();
        for name in [
            "canonical_model_id",
            "context_window",
            "max_context_window",
            "max_tokens",
            "max_output_tokens",
            "input_modalities",
            "supported_input_modalities",
            "output_modalities",
            "supported_reasoning_levels",
            "default_reasoning_level",
        ] {
            if let Some(value) = model.get(name).filter(|value| !value.is_null()) {
                let normalized = if name == "supported_reasoning_levels" {
                    value
                        .as_array()
                        .map(|levels| {
                            let efforts = levels
                                .iter()
                                .filter_map(|level| {
                                    level.get("effort").and_then(serde_json::Value::as_str)
                                })
                                .collect::<BTreeSet<_>>();
                            serde_json::json!(efforts)
                        })
                        .unwrap_or_else(|| value.clone())
                } else if name.ends_with("modalities") {
                    value
                        .as_array()
                        .map(|items| {
                            serde_json::json!(items
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .collect::<BTreeSet<_>>())
                        })
                        .unwrap_or_else(|| value.clone())
                } else {
                    value.clone()
                };
                fields.insert(name.into(), normalized);
            }
        }
        model_map.insert(id.to_string(), fields);
    }
    let plugin_list = plugins
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .ok_or("Core health check requires the plugin registry")?;
    let mut plugin_map = BTreeMap::new();
    for plugin in plugin_list {
        if plugin
            .get("registered")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        {
            continue;
        }
        let id = plugin
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("Registered plugin has no ID")?;
        plugin_map.insert(
            id.to_string(),
            plugin.get("effective_enabled").cloned().unwrap_or_default(),
        );
    }
    Ok(HealthSnapshot {
        models: model_map,
        plugins: plugin_map,
    })
}

async fn read_health(config: &GuiConfigFile) -> Result<HealthSnapshot, String> {
    let client = management_http_client()?;
    let response = client
        .get(format!(
            "{}/v1/models",
            managed_core_loopback_origin(config.port)
        ))
        .timeout(Duration::from_secs(4))
        .query(&[("client_version", "pi")])
        .bearer_auth(effective_agent_api_key(config))
        .send()
        .await
        .map_err(|_| "Model catalog health request failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!("Model catalog health HTTP {}", response.status()));
    }
    let models = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "Invalid model catalog health response".to_string())?;
    let response = client
        .get(management_endpoint(config, "plugins")?)
        .timeout(Duration::from_secs(4))
        .header("Authorization", management_authorization(config)?)
        .send()
        .await
        .map_err(|_| "Plugin health request failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!("Plugin health HTTP {}", response.status()));
    }
    let plugins = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "Invalid plugin health response".to_string())?;
    parse_health(&models, &plugins)
}

fn validate_health(before: &HealthSnapshot, after: &HealthSnapshot) -> Result<(), String> {
    for (id, fields) in &before.models {
        let Some(current) = after.models.get(id) else {
            return Err(format!("Updated core lost model route {id}"));
        };
        for (key, value) in fields {
            if current.get(key) != Some(value) {
                return Err(format!("Updated core changed {key} for {id}; review capability changes before updating"));
            }
        }
    }
    for (id, enabled) in &before.plugins {
        if after.plugins.get(id) != Some(enabled) {
            return Err(format!("Updated core lost registered plugin {id}"));
        }
    }
    Ok(())
}

async fn retry_health<F, Fut, A>(mut check: F, alive: A, timeout: Duration) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
    A: Fn() -> Result<(), String>,
{
    let deadline = Instant::now() + timeout;
    loop {
        alive()?;
        let result = check().await;
        if result.is_ok() || Instant::now() >= deadline {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_healthy(
    config: &GuiConfigFile,
    baseline: &HealthSnapshot,
    process: &CoreProcessState,
) -> Result<(), String> {
    retry_health(
        || async {
            read_health(config)
                .await
                .and_then(|after| validate_health(baseline, &after))
        },
        || {
            if current_core_status(Some(process), Some(config.port))?.running {
                Ok(())
            } else {
                Err("Candidate core exited during health checks".into())
            }
        },
        Duration::from_secs(15),
    )
    .await
}

fn stop_if_running<F>(running: bool, stop: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    if running {
        stop()
    } else {
        Ok(())
    }
}

fn stop_candidate(process: &CoreProcessState, port: u16) -> Result<(), String> {
    let running = current_core_status(Some(process), Some(port))?.running;
    stop_if_running(running, || stop_core_process_inner(process))
}

fn backup_files(install: &Path, binary: &Path, backup: &Path) -> Result<(), String> {
    fs::create_dir(backup).map_err(|e| format!("Create core backup: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(backup, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    fs::copy(binary, backup.join("core-binary"))
        .map_err(|e| format!("Back up core binary: {e}"))?;
    for name in [CORE_METADATA_FILE, PROVENANCE_FILE] {
        if install.join(name).exists() {
            fs::copy(install.join(name), backup.join(name)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn restore_files(install: &Path, binary: &Path, backup: &Path) -> Result<(), String> {
    copy_core_file_replace(&backup.join("core-binary"), binary)?;
    for name in [CORE_METADATA_FILE, PROVENANCE_FILE] {
        if backup.join(name).exists() {
            copy_core_file_replace(&backup.join(name), &install.join(name))?;
        } else if install.join(name).exists() {
            fs::remove_file(install.join(name)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// Exercise replacement and restoration with injected lifecycle operations in tests.
fn replace_and_check<F, R>(
    install: &Path,
    binary: &Path,
    staged: &Path,
    backup: &Path,
    check: F,
    stop_failed: R,
) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
    R: FnOnce() -> Result<(), String>,
{
    backup_files(install, binary, backup)?;
    let result = copy_core_file_replace(staged, binary).and_then(|_| check());
    if let Err(error) = result {
        stop_failed().map_err(|stop| {
            format!(
                "{error}; could not stop candidate for rollback: {stop}; backup retained at {}",
                path_to_string(backup)
            )
        })?;
        restore_files(install, binary, backup).map_err(|rollback| {
            format!(
                "{error}; rollback failed: {rollback}; backup retained at {}",
                path_to_string(backup)
            )
        })?;
        return Err(format!("{error}; previous core files restored"));
    }
    Ok(())
}

pub(crate) async fn install_reviewed_core(
    app: tauri::AppHandle,
    window: tauri::Window,
    version: Option<String>,
) -> Result<CoreInstallResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let process = app.state::<CoreProcessState>();
        let _guard = lock_core_operation(process.inner())?;
        let config = app.state::<GuiConfigState>().snapshot()?;
        if config.reviewed_core_manifest_url.is_empty() { return Err("Reviewed core channel is not configured".into()); }
        if !current_core_status(Some(process.inner()), Some(config.port))?.running {
            return Err("Start the existing core before installing a reviewed update so its models and plugins can be checked.".into());
        }
        let state = app.state::<CoreDownloadState>();
        let token = CancellationToken::new();
        state.start(token.clone(), version.clone())?;
        let result = (|| {
            let manifest = tauri::async_runtime::block_on(fetch_manifest(&config))?;
            if version.as_ref().is_some_and(|v| normalize_version(v) != normalize_version(&manifest.tag)) {
                return Err("Reviewed release changed since the update check. Check again before installing.".into());
            }
            let asset = selected_asset(&manifest)?.clone();
            let install = core_install_dir()?;
            let binary = find_core_binary(&install).ok_or("Install an initial core before switching to reviewed updates")?;
            let work = core_base_dir()?.join(format!("reviewed-stage-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()));
            fs::create_dir(&work).map_err(|e| e.to_string())?;
            let outcome = (|| {
                let archive = work.join(if asset.os == "windows" { "core.zip" } else { "core.tar.gz" });
                let client = reviewed_http_client(&config)?;
                tauri::async_runtime::block_on(download_asset_inner(&client, &asset.url, &archive, Some(asset.size), Some(&asset.sha256), &window, state.inner(), &token))?;
                ensure_not_cancelled(&token, Some(&archive))?;
                let unpacked = work.join("unpacked");
                fs::create_dir(&unpacked).map_err(|e| e.to_string())?;
                if asset.os == "windows" { extract_zip(&archive, &unpacked)?; } else { extract_tar_gz(&archive, &unpacked)?; }
                let staged = find_core_binary(&unpacked).ok_or("Reviewed archive has no core binary")?;
                let binary_sha256 = sha256_file(&staged)?;
                // The existing running core supplies the baseline. No credentials
                // are copied to staging or sent to any release server.
                let baseline = tauri::async_runtime::block_on(read_health(&config))?;
                ensure_not_cancelled(&token, None)?;
                state.progress(&window, "Verify reviewed core and preserve rollback", asset.size, Some(asset.size), false);
                stop_core_process_inner(process.inner())?;
                let backup = core_base_dir()?.join(format!("reviewed-backup-{}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(), &manifest.commit[..12]));
                let replaced = replace_and_check(&install, &binary, &staged, &backup, || {
                    start_core_process_preserving_config(process.inner(), &config)?;
                    tauri::async_runtime::block_on(wait_healthy(&config, &baseline, process.inner()))?;
                    write_core_metadata(&install, &CoreMetadata { version: normalize_version(&manifest.tag), asset_name: asset.url.rsplit('/').next().unwrap().into(), installed_at_unix: unix_now() })?;
                    let provenance = ReviewedCoreProvenance { manifest_url: config.reviewed_core_manifest_url.clone(), repository: manifest.repository.clone(), tag: manifest.tag.clone(), commit: manifest.commit.clone(), archive_sha256: asset.sha256.clone(), binary_sha256, installed_at_unix: unix_now(), backup_dir: path_to_string(&backup) };
                    fs::write(install.join(PROVENANCE_FILE), serde_json::to_vec_pretty(&provenance).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                    Ok(())
                }, || stop_candidate(process.inner(), config.port));
                if replaced.is_err() && !current_core_status(Some(process.inner()), Some(config.port))?.running {
                    start_core_process_preserving_config(process.inner(), &config).map_err(|e| format!("{}; restart after rollback failed: {e}", replaced.as_ref().unwrap_err()))?;
                }
                replaced?;
                Ok(CoreInstallResult { version: normalize_version(&manifest.tag), asset_name: asset.url.rsplit('/').next().unwrap().into(), install_dir: path_to_string(&install), binary_path: Some(path_to_string(&binary)) })
            })();
            let _ = fs::remove_dir_all(&work);
            outcome
        })();
        if let Ok(status) = current_core_status(Some(process.inner()), Some(config.port)) { emit_core_status(&app, &status); }
        state.finish(&window, result.clone());
        result
    }).await.map_err(|e| format!("Reviewed update worker failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> ReviewedCoreManifest {
        serde_json::from_value(json!({
            "schema_version":1,"repository":"example/CLIProxyAPI","tag":"v7.3.9-review.abc1234",
            "commit":"a".repeat(40),"assets":[{"os":"darwin","arch":"arm64",
                "url":"https://github.com/example/CLIProxyAPI/releases/download/v7.3.9-review.abc1234/core.tar.gz",
                "sha256":"b".repeat(64),"size":42000}]
        })).unwrap()
    }

    #[test]
    fn manifest_requires_pinned_repository_tag_hash_and_platform() {
        assert!(validate_manifest(&manifest()).is_ok());
        assert!(validate_download_chunk_size(4, 6, Some(10)).is_ok());
        assert!(validate_download_chunk_size(4, 7, Some(10)).is_err());
        assert!(validate_download_chunk_size(u64::MAX, 1, None).is_err());
        let invalid = [
            ("url", json!("https://github.com/other/CLIProxyAPI/releases/download/v7.3.9-review.abc1234/core.tar.gz")),
            ("url", json!("https://github.com/example/CLIProxyAPI/releases/latest/download/core.tar.gz")),
            ("url", json!("https://github.com/example/CLIProxyAPI/releases/download/v7.3.9-review.abc1234/../core.tar.gz")),
            ("url", json!("http://github.com/example/CLIProxyAPI/releases/download/v7.3.9-review.abc1234/core.tar.gz")),
            ("url", json!("https://github.com/example/CLIProxyAPI/releases/download/v7.3.9-review.abc1234/%2e%2e.tar.gz")),
            ("sha256", json!("")), ("sha256", json!("z".repeat(64))),
            ("size", json!(0)), ("size", json!(513u64*1024*1024)), ("arch", json!("other")),
        ];
        for (key, value) in invalid {
            let mut raw = serde_json::to_value(manifest()).unwrap();
            raw["assets"][0][key] = value;
            assert!(
                validate_manifest(&serde_json::from_value(raw).unwrap()).is_err(),
                "{key}"
            );
        }
        let mut raw = serde_json::to_value(manifest()).unwrap();
        raw["assets"][0].as_object_mut().unwrap().remove("sha256");
        assert!(serde_json::from_value::<ReviewedCoreManifest>(raw).is_err());
        let mut value = manifest();
        value.assets.push(value.assets[0].clone());
        assert!(validate_manifest(&value).is_err());
        let mut value = manifest();
        value.commit = "HEAD".into();
        assert!(validate_manifest(&value).is_err());
    }

    #[test]
    fn channel_round_trips_and_guards_official_updates() {
        let mut config = GuiConfigFile::default();
        assert!(reviewed_app_update_guard(&config).is_ok());
        assert!(reviewed_official_core_guard(&config).is_ok());
        config.reviewed_core_manifest_url =
            "https://raw.githubusercontent.com/example/CLIProxyAPI/reviewed-channel/core.json"
                .into();
        let encoded = toml::to_string(&config).unwrap();
        let restored: GuiConfigFile = toml::from_str(&encoded).unwrap();
        assert_eq!(
            restored.reviewed_core_manifest_url,
            config.reviewed_core_manifest_url
        );
        assert!(reviewed_app_update_guard(&restored).is_err());
        assert!(reviewed_official_core_guard(&restored).is_err());
        for invalid in [
            "http://example.org/core.json",
            "https://user:secret@example.org/core.json",
            "https://example.org/core.json?token=secret",
            "https://example.org:443/core.json#fragment",
        ] {
            assert!(validate_https_url(invalid).is_err());
        }
    }

    #[test]
    fn rich_catalog_and_registered_plugins_must_survive() {
        let catalog = json!({"models":[{"slug":"work/example","canonical_model_id":"gpt-example",
            "context_window":1000000,"supported_reasoning_levels":[{"effort":"high"},{"effort":"max"}],
            "input_modalities":["text","image"],"max_tokens":128000}]});
        let plugins =
            json!({"plugins":[{"id":"binding","registered":true,"effective_enabled":true}]});
        let before = parse_health(&catalog, &plugins).unwrap();
        assert!(validate_health(&before, &parse_health(&catalog, &plugins).unwrap()).is_ok());
        let mut reordered = catalog.clone();
        reordered["models"][0]["supported_reasoning_levels"] =
            json!([{"effort":"max","description":"Updated label"},{"effort":"high"}]);
        reordered["models"][0]["input_modalities"] = json!(["image", "text"]);
        assert!(validate_health(&before, &parse_health(&reordered, &plugins).unwrap()).is_ok());
        let mut dropped = catalog.clone();
        dropped["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("supported_reasoning_levels");
        assert!(validate_health(&before, &parse_health(&dropped, &plugins).unwrap()).is_err());
        let dropped = json!({"models":[]});
        assert!(validate_health(&before, &parse_health(&dropped, &plugins).unwrap()).is_err());
        let disabled =
            json!({"plugins":[{"id":"binding","registered":false,"effective_enabled":false}]});
        assert!(validate_health(&before, &parse_health(&catalog, &disabled).unwrap()).is_err());
    }

    #[test]
    fn health_check_retries_partial_registration_but_stops_on_process_exit() {
        let attempts = std::cell::Cell::new(0);
        tauri::async_runtime::block_on(retry_health(
            || {
                attempts.set(attempts.get() + 1);
                std::future::ready(if attempts.get() < 2 {
                    Err("plugin route still registering".into())
                } else {
                    Ok(())
                })
            },
            || Ok(()),
            Duration::from_secs(2),
        ))
        .unwrap();
        assert_eq!(attempts.get(), 2);
        let result = tauri::async_runtime::block_on(retry_health(
            || std::future::ready(Ok(())),
            || Err("candidate exited".into()),
            Duration::from_secs(2),
        ));
        assert_eq!(result.unwrap_err(), "candidate exited");
    }

    fn test_dir(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "reviewed-core-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn failed_health_restores_binary_metadata_and_preserves_config_plugins() {
        let dir = test_dir("rollback");
        let binary = dir.join("core");
        let staged = dir.join("candidate");
        let backup = dir.join("backup");
        fs::write(&binary, b"previous").unwrap();
        fs::write(&staged, b"candidate").unwrap();
        fs::write(dir.join(CORE_METADATA_FILE), b"old-metadata").unwrap();
        fs::write(dir.join(CORE_CONFIG_FILE), b"user config").unwrap();
        fs::create_dir(dir.join("plugins")).unwrap();
        fs::write(dir.join("plugins/binding"), b"keep").unwrap();
        let result = replace_and_check(
            &dir,
            &binary,
            &staged,
            &backup,
            || {
                assert_eq!(fs::read(&binary).unwrap(), b"candidate");
                fs::write(dir.join(CORE_METADATA_FILE), b"new-metadata").unwrap();
                fs::write(dir.join(PROVENANCE_FILE), b"new-provenance").unwrap();
                Err("health failed".into())
            },
            || stop_if_running(false, || Err("already stopped".into())),
        );
        assert!(result.unwrap_err().contains("previous core files restored"));
        assert_eq!(fs::read(&binary).unwrap(), b"previous");
        assert_eq!(
            fs::read(dir.join(CORE_METADATA_FILE)).unwrap(),
            b"old-metadata"
        );
        assert!(!dir.join(PROVENANCE_FILE).exists());
        assert_eq!(
            fs::read(dir.join(CORE_CONFIG_FILE)).unwrap(),
            b"user config"
        );
        assert_eq!(fs::read(dir.join("plugins/binding")).unwrap(), b"keep");
        assert_eq!(fs::read(backup.join("core-binary")).unwrap(), b"previous");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_replacement_and_failed_stop_keep_recovery_material() {
        let dir = test_dir("failure");
        let binary = dir.join("core");
        let backup = dir.join("backup");
        fs::write(&binary, b"previous").unwrap();
        let result = replace_and_check(
            &dir,
            &binary,
            &dir.join("missing"),
            &backup,
            || panic!("must not check a failed replacement"),
            || Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&binary).unwrap(), b"previous");
        let staged = dir.join("candidate");
        fs::write(&staged, b"candidate").unwrap();
        let backup = dir.join("backup2");
        let result = replace_and_check(
            &dir,
            &binary,
            &staged,
            &backup,
            || Err("start failed".into()),
            || Err("stop failed".into()),
        );
        assert!(result.unwrap_err().contains("backup retained"));
        assert_eq!(fs::read(backup.join("core-binary")).unwrap(), b"previous");
        fs::remove_dir_all(dir).unwrap();
    }
}

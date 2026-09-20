#[cfg(target_os = "windows")]
use super::windows_explorer_executable;
use super::{
    auth_dir_path_for_core, configure_background_command, core_install_dir, core_logs_dir_path,
    core_origin, current_core_tls_settings, is_hashed_management_secret_key, open_oauth_url_inner,
    path_to_string, truncate_for_error, GuiConfigFile, GuiConfigState,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::LazyLock,
    time::Duration,
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthStartResult {
    url: String,
    state: Option<String>,
    opened: bool,
    open_error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthStatusResult {
    status: String,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OAuthStartApiResponse {
    url: Option<String>,
    state: Option<String>,
    error: Option<String>,
    #[serde(rename = "error_message")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct OAuthStatusApiResponse {
    status: Option<String>,
    error: Option<String>,
    #[serde(rename = "error_message")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagementRequest {
    method: String,
    path: String,
    query: Option<HashMap<String, String>>,
    body: Option<serde_json::Value>,
    #[serde(rename = "timeoutMs")]
    timeout_ms: Option<u64>,
}

#[tauri::command]
pub(crate) async fn management_request(
    app: tauri::AppHandle,
    gui_config_state: tauri::State<'_, GuiConfigState>,
    request: ManagementRequest,
) -> Result<serde_json::Value, String> {
    let config = gui_config_state.snapshot()?;
    let path = request.path.trim().trim_start_matches('/');
    if request.method.trim().eq_ignore_ascii_case("POST") && path.starts_with("plugin-store/") && path.ends_with("/install") {
        // Plugin replacement must not race core replacement or a manual restart.
        return tauri::async_runtime::spawn_blocking(move || {
            use tauri::Manager;
            let _guard = super::lock_core_operation(app.state::<super::CoreProcessState>().inner())?;
            if app.state::<super::AppUpdateState>().snapshot().running {
                return Err("Wait for the app update to finish before updating plugins".into());
            }
            tauri::async_runtime::block_on(async {
                let current = management_request_inner(&config, ManagementRequest {
                    method: "GET".into(), path: "plugins".into(), query: None, body: None, timeout_ms: Some(5000),
                }).await?;
                let id = request.path.trim().trim_start_matches('/').strip_prefix("plugin-store/")
                    .and_then(|path| path.strip_suffix("/install")).unwrap_or_default();
                validate_plugin_update_enabled(&current, id)?;
                management_request_inner(&config, request).await
            })
        }).await.map_err(|error| format!("Plugin update worker failed: {error}"))?;
    }
    management_request_inner(&config, request).await
}

fn validate_plugin_update_enabled(current: &serde_json::Value, id: &str) -> Result<(), String> {
    let enabled = current.get("plugins_enabled").and_then(serde_json::Value::as_bool) == Some(true)
        && current.get("plugins").and_then(serde_json::Value::as_array).is_some_and(|plugins| plugins.iter().any(|plugin|
            plugin.get("id").and_then(serde_json::Value::as_str) == Some(id)
                && plugin.get("enabled").and_then(serde_json::Value::as_bool) == Some(true)));
    if enabled { Ok(()) } else { Err("Plugin updates require an already enabled plugin; no enable setting was changed".into()) }
}

async fn management_request_inner(config: &GuiConfigFile, request: ManagementRequest) -> Result<serde_json::Value, String> {
    let method = match request.method.trim().to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        _ => return Err("不支持的管理 API 请求方法".to_string()),
    };
    let path = request.path.trim();
    if path.is_empty() || path.contains("://") || path.contains("..") {
        return Err("无效的管理 API 路径".to_string());
    }

    let client = management_http_client()?;
    let mut builder = client
        .request(method, management_endpoint(&config, path)?)
        .header("Authorization", management_authorization(&config)?);
    if let Some(timeout_ms) = request.timeout_ms {
        let max_timeout = if request.method.trim().eq_ignore_ascii_case("POST") && path.trim_start_matches('/').starts_with("plugin-store/") && path.ends_with("/install") { 300_000 } else { 120_000 };
        builder = builder.timeout(Duration::from_millis(timeout_ms.clamp(1_000, max_timeout)));
    }
    if let Some(query) = request.query {
        builder = builder.query(&query);
    }
    if let Some(body) = request.body {
        builder = builder.json(&body);
    }

    let response = builder
        .send()
        .await
        .map_err(|err| format_management_request_error("请求管理 API 失败", &err))?;
    read_management_value(response).await
}

#[tauri::command]
pub(crate) async fn upload_auth_file(
    gui_config_state: tauri::State<'_, GuiConfigState>,
    name: String,
    data: Vec<u8>,
) -> Result<serde_json::Value, String> {
    let name = name.trim().to_string();
    if name.is_empty() || !name.to_ascii_lowercase().ends_with(".json") {
        return Err("凭证文件名必须以 .json 结尾".to_string());
    }

    let config = gui_config_state.snapshot()?;
    let client = management_http_client()?;
    let mut query = HashMap::new();
    query.insert("name".to_string(), name);
    let response = client
        .post(management_endpoint(&config, "auth-files")?)
        .header("Authorization", management_authorization(&config)?)
        .query(&query)
        .header("Content-Type", "application/json")
        .body(data)
        .send()
        .await
        .map_err(|err| format_management_request_error("上传凭证文件失败", &err))?;
    read_management_value(response).await
}

#[tauri::command]
pub(crate) fn open_auth_files_directory(
    gui_config_state: tauri::State<'_, GuiConfigState>,
) -> Result<(), String> {
    let config = gui_config_state.snapshot()?;
    let install_dir = core_install_dir()?;
    let auth_dir = auth_dir_path_for_core(&config.auth_dir, &install_dir);
    fs::create_dir_all(&auth_dir)
        .map_err(|error| format!("创建凭证目录失败 {}: {error}", path_to_string(&auth_dir)))?;
    open_directory_in_file_manager(&auth_dir)
}

#[tauri::command]
pub(crate) fn open_core_logs_directory(
    gui_config_state: tauri::State<'_, GuiConfigState>,
) -> Result<(), String> {
    let config = gui_config_state.snapshot()?;
    let install_dir = core_install_dir()?;
    let logs_dir = core_logs_dir_path(&config.auth_dir, &install_dir);
    fs::create_dir_all(&logs_dir)
        .map_err(|error| format!("创建日志目录失败 {}: {error}", path_to_string(&logs_dir)))?;

    open_directory_in_file_manager(&logs_dir)
}

fn open_directory_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = Command::new(windows_explorer_executable());
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");

    command.arg(path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_background_command(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("打开目录失败 {}: {error}", path_to_string(path)))
}

#[tauri::command]
pub(crate) async fn start_oauth_login(
    app: tauri::AppHandle,
    gui_config_state: tauri::State<'_, GuiConfigState>,
    provider: String,
    browser: Option<String>,
) -> Result<OAuthStartResult, String> {
    super::network_proxy::prepare_oauth(&app).await;
    let config = gui_config_state.snapshot()?;
    let provider_key = normalize_management_oauth_provider(&provider)?;
    let client = management_http_client()?;
    let mut request = client
        .get(management_endpoint(
            &config,
            &format!("{provider_key}-auth-url"),
        )?)
        .header("Authorization", management_authorization(&config)?);
    if management_oauth_uses_webui_callback(&provider_key) {
        request = request.query(&[("is_webui", "true")]);
    }
    let response = request
        .send()
        .await
        .map_err(|err| format_management_request_error("请求 OAuth 登录链接失败", &err))?;
    let payload = read_management_json::<OAuthStartApiResponse>(response).await?;
    if let Some(error) = payload
        .error
        .or(payload.error_message)
        .filter(|value| !value.trim().is_empty())
    {
        return Err(error);
    }
    let url = payload
        .url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "内核未返回 OAuth 登录链接".to_string())?;
    let state = payload
        .state
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let (opened, open_error) = match open_oauth_url_inner(&app, &url, browser.as_deref()) {
        Ok(()) => (true, None),
        Err(error) => (false, Some(error)),
    };

    Ok(OAuthStartResult {
        url,
        state,
        opened,
        open_error,
    })
}

#[tauri::command]
pub(crate) async fn get_oauth_status(
    gui_config_state: tauri::State<'_, GuiConfigState>,
    state: String,
) -> Result<OAuthStatusResult, String> {
    let state = state.trim().to_string();
    if state.is_empty() {
        return Err("OAuth state 不能为空".to_string());
    }
    let config = gui_config_state.snapshot()?;
    let client = management_http_client()?;
    let response = client
        .get(management_endpoint(&config, "get-auth-status")?)
        .header("Authorization", management_authorization(&config)?)
        .query(&[("state", state)])
        .send()
        .await
        .map_err(|err| format_management_request_error("查询 OAuth 状态失败", &err))?;
    let payload = read_management_json::<OAuthStatusApiResponse>(response).await?;
    let status = payload
        .status
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "wait".to_string());
    Ok(OAuthStatusResult {
        status,
        error: payload
            .error
            .or(payload.error_message)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

#[tauri::command]
pub(crate) async fn submit_oauth_callback(
    gui_config_state: tauri::State<'_, GuiConfigState>,
    provider: String,
    redirect_url: String,
) -> Result<(), String> {
    let redirect_url = redirect_url.trim().to_string();
    if redirect_url.is_empty() {
        return Err("回调链接不能为空".to_string());
    }
    let config = gui_config_state.snapshot()?;
    let provider_key = normalize_management_oauth_provider(&provider)?;
    let client = management_http_client()?;
    let body = serde_json::json!({
        "provider": provider_key,
        "redirect_url": redirect_url,
    });
    let response = client
        .post(management_endpoint(&config, "oauth-callback")?)
        .header("Authorization", management_authorization(&config)?)
        .json(&body)
        .send()
        .await
        .map_err(|err| format_management_request_error("提交 OAuth 回调失败", &err))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format_management_request_error("读取 OAuth 回调响应失败", &err))?;
    if !status.is_success() {
        return Err(format_management_error(status.as_u16(), &text));
    }
    Ok(())
}

pub(crate) fn management_http_client() -> Result<reqwest::Client, String> {
    static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|err| format_management_request_error("创建管理 API 客户端失败", &err))
    });
    CLIENT.as_ref().cloned().map_err(Clone::clone)
}

pub(crate) fn format_management_request_error(
    action: &str,
    error: &(dyn Error + 'static),
) -> String {
    let mut messages = Vec::new();
    let mut current = Some(error);

    while let Some(error) = current {
        let message = error.to_string();
        if !message.is_empty()
            && messages
                .last()
                .map(|previous| previous != &message)
                .unwrap_or(true)
        {
            messages.push(message);
        }
        current = error.source();
    }

    if messages.is_empty() {
        action.to_string()
    } else {
        format!("{action}: {}", messages.join(": "))
    }
}

pub(crate) fn management_authorization(config: &GuiConfigFile) -> Result<String, String> {
    let secret_key = config.management_secret_key.trim();
    if secret_key.is_empty() || is_hashed_management_secret_key(secret_key) {
        return Err("管理接口不可用：没有可用的明文管理密钥".to_string());
    }
    Ok(format!("Bearer {secret_key}"))
}

pub(crate) fn management_endpoint(config: &GuiConfigFile, path: &str) -> Result<String, String> {
    if config.port == 0 {
        return Err("内核端口无效".to_string());
    }
    let path = path.trim_start_matches('/');
    let origin = core_origin(
        &config.host,
        config.port,
        current_core_tls_settings()?.enabled,
    );
    Ok(format!("{origin}/v0/management/{path}"))
}

fn normalize_management_oauth_provider(provider: &str) -> Result<String, String> {
    let key = provider.trim().to_ascii_lowercase().replace('_', "-");
    let key = match key.as_str() {
        "claude" | "anthropic" => "anthropic".to_string(),
        "anti-gravity" => "antigravity".to_string(),
        "cognition" => "devin".to_string(),
        "grok" | "x-ai" | "x.ai" => "xai".to_string(),
        other => other.to_string(),
    };
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("无效的 OAuth 提供商".to_string());
    }
    Ok(key)
}

fn management_oauth_uses_webui_callback(provider_key: &str) -> bool {
    matches!(provider_key, "codex" | "anthropic" | "antigravity" | "xai" | "devin")
}

async fn read_management_json<T>(response: reqwest::Response) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format_management_request_error("读取管理 API 响应失败", &err))?;
    if !status.is_success() {
        return Err(format_management_error(status.as_u16(), &text));
    }
    if text.trim().is_empty() {
        return Err("管理 API 返回了空响应".to_string());
    }
    serde_json::from_str::<T>(&text).map_err(|err| {
        format!(
            "解析管理 API 响应失败: {err}; body={}",
            truncate_for_error(&text)
        )
    })
}

pub(crate) async fn read_management_value(
    response: reqwest::Response,
) -> Result<serde_json::Value, String> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format_management_request_error("读取管理 API 响应失败", &err))?;
    if !status.is_success() {
        return Err(format_management_error(status.as_u16(), &text));
    }
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => Ok(value),
        Err(_) => Ok(serde_json::Value::String(text)),
    }
}

pub(crate) async fn read_management_text(response: reqwest::Response) -> Result<String, String> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format_management_request_error("读取管理 API 响应失败", &err))?;
    if !status.is_success() {
        return Err(format_management_error(status.as_u16(), &text));
    }
    Ok(text)
}

fn format_management_error(status: u16, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|item| item.as_str())
            .or_else(|| value.get("message").and_then(|item| item.as_str()))
        {
            let message = message.trim();
            if !message.is_empty() {
                return format!("管理 API 错误 ({status}): {message}");
            }
        }
    }
    let body = body.trim();
    if body.is_empty() {
        format!("管理 API 错误 ({status})")
    } else {
        format!("管理 API 错误 ({status}): {}", truncate_for_error(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn devin_oauth_uses_the_management_callback_flow() {
        for provider in ["Devin", " cognition "] {
            let key = normalize_management_oauth_provider(provider).unwrap();
            assert_eq!(key, "devin");
            assert!(management_oauth_uses_webui_callback(&key));
        }
    }

    #[test]
    fn core_logs_follow_the_default_auth_directory() {
        let base_dir = PathBuf::from("test-base");
        let install_dir = base_dir.join("cpa-core");

        assert_eq!(
            core_logs_dir_path("../oauth", &install_dir),
            base_dir.join("oauth").join("logs")
        );
    }

    #[test]
    fn core_logs_follow_a_custom_auth_directory() {
        let install_dir = PathBuf::from("test-base").join("cpa-core");
        let auth_dir = PathBuf::from("custom-auth");

        assert_eq!(
            core_logs_dir_path(auth_dir.to_str().unwrap(), &install_dir),
            install_dir.join(auth_dir).join("logs")
        );
    }
}


#[cfg(test)]
mod plugin_update_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plugin_install_preflight_does_not_enable_disabled_or_unknown_plugins() {
        let active = json!({"plugins_enabled":true,"plugins":[{"id":"example","enabled":true}]});
        assert!(validate_plugin_update_enabled(&active,"example").is_ok());
        assert!(validate_plugin_update_enabled(&active,"other").is_err());
        assert!(validate_plugin_update_enabled(&json!({"plugins_enabled":true,"plugins":[{"id":"example","enabled":false}]}),"example").is_err());
        assert!(validate_plugin_update_enabled(&json!({"plugins_enabled":false,"plugins":[{"id":"example","enabled":true}]}),"example").is_err());
    }
}

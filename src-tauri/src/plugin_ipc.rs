use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    plugin::runtime_pipe_name,
    protocol::{hello_result, parse_request, PluginRequest},
    WorkerState,
};

pub async fn serve(state: Arc<WorkerState>) -> Result<(), String> {
    #[cfg(windows)]
    {
        serve_windows(state).await
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Err("插件 IPC 仅支持 Windows。".to_string())
    }
}

#[cfg(windows)]
async fn serve_windows(state: Arc<WorkerState>) -> Result<(), String> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = runtime_pipe_name()?;
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .map_err(|error| format!("创建插件管道失败：{error}"))?;

    loop {
        if state.quitting.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        server
            .connect()
            .await
            .map_err(|error| format!("等待插件管道连接失败：{error}"))?;
        let connected = server;
        server = ServerOptions::new()
            .create(&pipe_name)
            .map_err(|error| format!("重建插件管道失败：{error}"))?;

        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_client(state, connected).await {
                eprintln!("插件 IPC 会话结束：{error}");
            }
        });
    }
    Ok(())
}

#[cfg(windows)]
async fn handle_client(
    state: Arc<WorkerState>,
    client: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
) -> Result<(), String> {
    let (reader, mut writer) = tokio::io::split(client);
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let response = match parse_request(&line) {
            Ok(request) => dispatch(Arc::clone(&state), request).await,
            Err(error) => json!({
                "id": "",
                "ok": false,
                "error": error
            }),
        };
        let mut payload = serde_json::to_string(&response).map_err(|error| error.to_string())?;
        payload.push('\n');
        writer
            .write_all(payload.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        writer.flush().await.map_err(|error| error.to_string())?;
        if state.quitting.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }
    Ok(())
}

pub async fn dispatch(state: Arc<WorkerState>, request: PluginRequest) -> Value {
    let id = request.id.clone();
    let result = match request.cmd.as_str() {
        "hello" => Ok(hello_result()),
        "configure" => state.configure(request.params.as_ref()).await,
        "status" => state.status_value(),
        "apply" => {
            let state = Arc::clone(&state);
            match tokio::task::spawn_blocking(move || state.apply()).await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            }
        }
        "pause" => {
            let state = Arc::clone(&state);
            match tokio::task::spawn_blocking(move || state.pause()).await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            }
        }
        "restore" => {
            let state = Arc::clone(&state);
            match tokio::task::spawn_blocking(move || state.restore()).await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            }
        }
        "shutdown" => Ok(state.shutdown()),
        other => Err(format!("未知命令：{other}")),
    };
    match result {
        Ok(value) => json!({ "id": id, "ok": true, "result": value }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkerState;
    use serde_json::json;
    use uuid::Uuid;

    fn test_state() -> Arc<WorkerState> {
        let root = std::env::temp_dir().join(format!("notion-ipc-{}", Uuid::new_v4()));
        Arc::new(WorkerState::load_from(root).unwrap())
    }

    #[tokio::test]
    async fn hello_and_status_and_unknown_commands() {
        let state = test_state();
        let hello = dispatch(
            Arc::clone(&state),
            PluginRequest {
                id: "1".to_string(),
                cmd: "hello".to_string(),
                params: None,
            },
        )
        .await;
        assert_eq!(hello["ok"], true);
        assert_eq!(hello["result"]["pluginProtocol"], 2);
        assert_eq!(hello["result"]["pluginId"], "notion");
        assert!(hello["result"]["capabilities"]["blobInject"]
            .as_bool()
            .unwrap());

        let status = dispatch(
            Arc::clone(&state),
            PluginRequest {
                id: "2".to_string(),
                cmd: "status".to_string(),
                params: None,
            },
        )
        .await;
        assert_eq!(status["result"]["configured"], false);
        assert_eq!(status["result"]["message"], "尚未配置背景");

        let unknown = dispatch(
            Arc::clone(&state),
            PluginRequest {
                id: "3".to_string(),
                cmd: "open-ui".to_string(),
                params: None,
            },
        )
        .await;
        assert_eq!(unknown["ok"], false);
        assert!(unknown["error"].as_str().unwrap().contains("未知命令"));
    }

    #[tokio::test]
    async fn pause_restore_and_shutdown() {
        let state = test_state();
        let pause = dispatch(
            Arc::clone(&state),
            PluginRequest {
                id: "7".to_string(),
                cmd: "pause".to_string(),
                params: None,
            },
        )
        .await;
        assert_eq!(pause["ok"], true);
        assert_eq!(pause["result"]["paused"], true);

        let restore_request = parse_request(r#"{"id":"8","cmd":"restore"}"#).unwrap();
        assert_eq!(restore_request.cmd, "restore");

        let apply = dispatch(
            Arc::clone(&state),
            PluginRequest {
                id: "4".to_string(),
                cmd: "apply".to_string(),
                params: None,
            },
        )
        .await;
        assert_eq!(apply["ok"], false);
        assert!(apply["error"].as_str().unwrap().contains("尚未配置背景"));

        let shutdown = dispatch(
            Arc::clone(&state),
            PluginRequest {
                id: "5".to_string(),
                cmd: "shutdown".to_string(),
                params: None,
            },
        )
        .await;
        assert_eq!(shutdown["ok"], true);
        assert_eq!(shutdown["result"]["shutdown"], true);
        assert_eq!(shutdown["result"]["keptTarget"], true);
        assert!(state.quitting.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn configure_rejects_non_loopback_without_fetch() {
        let state = test_state();
        let response = dispatch(
            state,
            PluginRequest {
                id: "6".to_string(),
                cmd: "configure".to_string(),
                params: Some(json!({
                    "schemaVersion": 1,
                    "revision": "bad",
                    "media": {
                        "url": "http://example.com/a.png",
                        "kind": "image",
                        "mimeType": "image/png",
                        "sha256": "a".repeat(64),
                        "byteSize": 8
                    },
                    "display": crate::models::DisplaySettings::default()
                })),
            },
        )
        .await;
        assert_eq!(response["ok"], false);
        assert!(response["error"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1 或 localhost"));
    }
}

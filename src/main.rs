/* Imports
  ###########################################################################
  ###########################################################################
  ########################## LSP Server #####################################
  ###########################################################################
  ###########################################################################
*/
use futures_util::{SinkExt, StreamExt};
use log::*;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::Arc,
};
use tokio::{net::TcpListener as TokioTcpListener, sync::Mutex};
use tokio_tungstenite::{accept_async, tungstenite::Message};

struct LanguageServerConfig {
    command: String,
    args: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LanguageType {
    Rust,
    Go,
    Python,
    JavaScript,
    TypeScript,
}
impl LanguageType {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" => Some(LanguageType::Rust),
            "go" => Some(LanguageType::Go),
            "python" => Some(LanguageType::Python),
            "javascript" | "js" => Some(LanguageType::JavaScript),
            "typescript" | "ts" => Some(LanguageType::TypeScript),
            _ => None,
        }
    }
}

#[derive(Deserialize, Debug)]
struct InitializeParams {
    #[serde(rename = "initializationOptions")]
    initialization_options: Option<InitOptions>,
}

#[derive(Deserialize, Debug)]
struct InitOptions {
    language: Option<String>,
}

#[derive(Deserialize, Debug)]
struct LspMessage {
    method: Option<String>,
    params: Option<Value>,
}

struct LspProcess {
    child: Child,
}

impl LspProcess {
    fn new(config: &LanguageServerConfig) -> Self {
        let mut command = Command::new(&config.command);
        if !config.args.is_empty() {
            command.args(&config.args);
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect(&format!(
                "Failed to start language server process: {}",
                config.command
            ));
        LspProcess { child }
    }
    fn stdin(&mut self) -> &mut std::process::ChildStdin {
        self.child.stdin.as_mut().expect("Failed to get stdin")
    }
}

impl Drop for LspProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn get_language_server_configs() -> HashMap<LanguageType, LanguageServerConfig> {
    let mut configs = HashMap::new();

    if cfg!(target_os = "windows") {
        configs.insert(
            LanguageType::Rust,
            LanguageServerConfig {
                command: "C:/Users/User/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/rust-analyzer.exe".to_string(),
                args: vec![],
            },
        );
        configs.insert(
            LanguageType::Go,
            LanguageServerConfig {
                command: "gopls".to_string(),
                args: vec![],
            },
        );
        configs.insert(
            LanguageType::JavaScript,
            LanguageServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
        configs.insert(
            LanguageType::TypeScript,
            LanguageServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
        configs.insert(
            LanguageType::Python,
            LanguageServerConfig {
                command: "pyright-langserver.cmd".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
    } else {
        // Linux/Mac configurations
        configs.insert(
            LanguageType::Rust,
            LanguageServerConfig {
                command: "rust-analyzer".to_string(),
                args: vec![],
            },
        );
        configs.insert(
            LanguageType::Go,
            LanguageServerConfig {
                command: "gopls".to_string(),
                args: vec![],
            },
        );
        configs.insert(
            LanguageType::JavaScript,
            LanguageServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
        configs.insert(
            LanguageType::TypeScript,
            LanguageServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
        configs.insert(
            LanguageType::Python,
            LanguageServerConfig {
                command: "pyright-langserver".to_string(),
                args: vec!["--stdio".to_string()],
            },
        );
    }

    configs
}

async fn handle_websocket(stream: tokio::net::TcpStream) {
    info!("New WebSocket connection");

    // Accept the WebSocket connection
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            error!("Error during WebSocket handshake: {:?}", e);
            return;
        }
    };
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let mut language_type = None;
    let mut first_message = None;

    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                debug!("Received initial message from client: {}", text);
                if let Ok(message) = serde_json::from_str::<LspMessage>(&text) {
                    if let Some(method) = &message.method {
                        if method == "initialize" {
                            if let Some(params) = &message.params {
                                if let Ok(init_params) =
                                    serde_json::from_value::<InitializeParams>(params.clone())
                                {
                                    if let Some(init_options) = init_params.initialization_options {
                                        if let Some(lang) = init_options.language {
                                            language_type = LanguageType::from_str(&lang);
                                            info!("Detected language: {:?}", language_type);
                                        }
                                    }
                                }
                            }
                            first_message = Some(text);
                            break;
                        }
                    }
                }
                if language_type.is_none() {
                    language_type = Some(LanguageType::Rust);
                    info!("Using default language: Rust");
                    first_message = Some(text);
                    break;
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client closed connection during initialization");
                return;
            }
            Ok(_) => {} // Ignore other message types
            Err(e) => {
                error!("Error receiving WebSocket message: {:?}", e);
                return;
            }
        }
    }
    let configs = get_language_server_configs();
    let lang_type = language_type.unwrap_or(LanguageType::Rust);

    let config = match configs.get(&lang_type) {
        Some(config) => config,
        None => {
            error!("Unsupported language: {:?}", lang_type);
            let _ = ws_sender.send(Message::Text(format!(
                "{{\"jsonrpc\": \"2.0\", \"id\": null, \"error\": {{\"code\": -32603, \"message\": \"Unsupported language: {:?}\"}}}}", 
                lang_type
            ).into())).await;
            return;
        }
    };

    info!(
        "Starting language server for {:?}: {} {:?}",
        lang_type, config.command, config.args
    );

    let lsp_process = Arc::new(Mutex::new(LspProcess::new(config)));
    let mut stderr = lsp_process.lock().await.child.stderr.take().unwrap();
    tokio::spawn(async move {
        let mut stderr_buf = [0u8; 1024];
        loop {
            match stderr.read(&mut stderr_buf) {
                Ok(0) => break,
                Ok(n) => {
                    let msg = String::from_utf8_lossy(&stderr_buf[..n]);
                    error!("{:?} language server stderr: {}", lang_type, msg);
                }
                Err(e) => {
                    error!("Failed to read stderr: {}", e);
                    break;
                }
            }
        }
    });
    let lsp_process_clone = lsp_process.clone();
    if let Some(message) = first_message {
        let mut lsp = lsp_process.lock().await;
        let content = message;
        let header = format!("Content-Length: {}\r\n\r\n", content.len());
        let stdin = lsp.stdin();
        let _ = stdin.write_all(header.as_bytes());
        let _ = stdin.write_all(content.as_bytes());
        let _ = stdin.flush();
    }
    let client_to_lsp = async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    debug!("Received from client: {}", text);
                    {
                        let mut lsp = lsp_process.lock().await;
                        let content = text;
                        let header = format!("Content-Length: {}\r\n\r\n", content.len());
                        let stdin = lsp.stdin();
                        let _ = stdin.write_all(header.as_bytes());
                        let _ = stdin.write_all(content.as_bytes());
                        let _ = stdin.flush();
                    }
                }
                Ok(Message::Binary(data)) => {
                    debug!("Received binary data from client: {} bytes", data.len());
                    {
                        let mut lsp = lsp_process.lock().await;
                        if let Err(e) = lsp.stdin().write_all(&data) {
                            error!("Failed to write binary data to LSP process: {}", e);
                            break;
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("CL closed connection");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Error receiving WebSocket message: {:?}", e);
                    break;
                }
            }
        }
        info!("CL -> LSP task ended");
    };
    use std::io::{BufRead, BufReader};
    let lsp_to_client = async move {
        let stdout = {
            let mut lsp = lsp_process_clone.lock().await;
            lsp.child.stdout.take().expect("Failed to get stdout")
        };
        let mut reader = BufReader::new(stdout);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);
        std::thread::spawn(move || {
            let mut line = String::new();
            let mut buffer = Vec::new();
            loop {
                let mut content_length: Option<usize> = None;
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => return, // EOF
                        Ok(_) => {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                break; // Empty line indicates end of headers
                            }
                            if trimmed.to_lowercase().starts_with("content-length:") {
                                if let Some(len_str) = trimmed.split(':').nth(1) {
                                    content_length = len_str.trim().parse::<usize>().ok();
                                }
                            }
                        }
                        Err(e) => {
                            error!("Error reading from LSP stdout: {}", e);
                            return;
                        }
                    }
                }
                if let Some(len) = content_length {
                    buffer.clear();
                    buffer.resize(len, 0);

                    match reader.read_exact(&mut buffer) {
                        Ok(_) => {
                            if let Ok(message) = String::from_utf8(buffer.clone()) {
                                if tx.blocking_send(message).is_err() {
                                    break;
                                }
                            } else {
                                error!("Failed to decode LSP message as UTF-8");
                            }
                        }
                        Err(e) => {
                            error!("Failed to read message body: {}", e);
                            break;
                        }
                    }
                } else {
                    error!("Missing Content-Length header");
                    break;
                }
            }
        });
        while let Some(message) = rx.recv().await {
            debug!("LSP -> CL: {}", message);
            if let Err(e) = ws_sender.send(Message::Text(message.into())).await {
                error!("WS FAIL client: {:?}", e);
                break;
            }
        }
        info!("LSP -> CLIENT. PROCESS FINISHED");
        let _ = ws_sender.close().await;
    };
    tokio::select! {
        _ = client_to_lsp => {
            info!("=================================================");
            info!("CL -> LSP task finished");
            info!("=================================================");
        },
        _ = lsp_to_client => {
            info!("=================================================");
            info!("LSP -> CL task finished");
            info!("=================================================");
        },
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_default_env()
        .filter(None, log::LevelFilter::Debug)
        .init();
    info!("=================================================");
    let listener = TokioTcpListener::bind("127.0.0.1:9003").await?;
    while let Ok((stream, addr)) = listener.accept().await {
        info!("CONN: {}", addr);
        tokio::spawn(async move {
            handle_websocket(stream).await;
        });
    }
    info!("=================================================");
    Ok(())
}

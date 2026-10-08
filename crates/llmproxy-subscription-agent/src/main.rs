use std::{path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use llmproxy_subscription_agent::doctor;

const USAGE: &str = "llmproxy-subscription-agent doctor [--codex-bin PATH]\nllmproxy-subscription-agent login --state-dir PATH\nllmproxy-subscription-agent serve [--backend codex|chatgpt-oauth] [--codex-bin PATH] --state-dir PATH --listen IP:PORT --key-file PATH --models MODEL[,MODEL] [--concurrency N] [--remote https://HOST --registration-key-file PATH [--name NAME]]\n\n默认后端为 codex，需要受控无会话扩展及本地 ChatGPT 登录，不自动切换。\n凭据目录必须为 0700，访问密钥文件必须为 0600。doctor 退出码：0 条件通过；1 诊断失败；2 缺失或未验证。";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args
        .first()
        .is_some_and(|arg| arg == "login" || arg == "serve")
    {
        return match run(&args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    let program = match args.as_slice() {
        [command] if command == "doctor" => PathBuf::from("codex"),
        [command, option, path] if command == "doctor" && option == "--codex-bin" => {
            PathBuf::from(path)
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match doctor::inspect(&program).await {
        Ok(report) => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            ExitCode::from(if report.backend_ready { 0 } else { 2 })
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[std::ffi::OsString]) -> Result<(), llmproxy_subscription_agent::Error> {
    use llmproxy_subscription_agent::{credentials::StateDirectory, oauth, service::Agent};
    let mut options = std::collections::HashMap::new();
    let (pairs, remainder) = args[1..].as_chunks::<2>();
    for pair in pairs {
        let key = pair[0].to_str().ok_or("选项无效")?;
        if ![
            "--backend",
            "--codex-bin",
            "--state-dir",
            "--listen",
            "--key-file",
            "--models",
            "--concurrency",
            "--remote",
            "--registration-key-file",
            "--name",
        ]
        .contains(&key)
            || options.insert(key, pair[1].clone()).is_some()
        {
            return Err("未知或重复选项".into());
        }
    }
    if !remainder.is_empty() {
        return Err("选项缺少值".into());
    }
    let get = |name| {
        options
            .get(name)
            .and_then(|value| value.to_str())
            .ok_or("缺少必需启动选项")
    };
    let backend = options
        .get("--backend")
        .and_then(|value| value.to_str())
        .unwrap_or("codex");
    if !["codex", "chatgpt-oauth"].contains(&backend) {
        return Err("后端必须为 codex 或 chatgpt-oauth".into());
    }
    if backend == "chatgpt-oauth" && options.contains_key("--codex-bin") {
        return Err("OAuth 后端不接受 --codex-bin".into());
    }
    if args[0] == "login" && options.keys().any(|key| *key != "--state-dir") {
        return Err("login 只接受 --state-dir".into());
    }
    if !options.contains_key("--remote")
        && (options.contains_key("--registration-key-file") || options.contains_key("--name"))
    {
        return Err("远程选项需要 --remote".into());
    }
    let directory = Arc::new(StateDirectory::open(PathBuf::from(get("--state-dir")?))?);
    if args[0] == "login" {
        return oauth::login(&directory).await;
    }
    let _lock = directory.lock()?;
    let key_path = PathBuf::from(get("--key-file")?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(&key_path)?;
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
            return Err("访问密钥文件必须为实际文件且权限 0600".into());
        }
    }
    let key = std::fs::read_to_string(key_path)?.trim().to_owned();
    let listen = get("--listen")?.parse()?;
    let models = get("--models")?.split(',').map(str::to_owned).collect();
    let concurrency = options
        .get("--concurrency")
        .and_then(|value| value.to_str())
        .unwrap_or("4")
        .parse()?;
    let agent = if backend == "codex" {
        let program = options
            .get("--codex-bin")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("codex"));
        Agent::codex(&program, concurrency, models, Duration::from_secs(300)).await?
    } else {
        Agent::new(
            oauth::Session::load(directory.clone())?,
            concurrency,
            models,
            Duration::from_secs(300),
        )?
    };
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop.cancel();
    });
    if options.contains_key("--remote") {
        let key_path = PathBuf::from(get("--registration-key-file")?);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::symlink_metadata(&key_path)?;
            if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
                return Err("注册密钥文件必须为实际文件且权限 0600".into());
            }
        }
        let remote = llmproxy_subscription_agent::reverse::Remote {
            url: get("--remote")?.parse()?,
            registration_key: std::fs::read_to_string(key_path)?.trim().to_owned(),
            name: options
                .get("--name")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        remote.validate()?;
        tokio::select! {
            result=llmproxy_subscription_agent::http::serve(agent.clone(),listen,key,shutdown.clone())=>{shutdown.cancel();result},
            result=llmproxy_subscription_agent::reverse::run(agent,directory,remote,shutdown.clone())=>{shutdown.cancel();result}
        }
    } else {
        llmproxy_subscription_agent::http::serve(agent, listen, key, shutdown).await
    }
}

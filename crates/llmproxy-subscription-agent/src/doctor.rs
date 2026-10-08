//! 区分接口检查、受控进程确认和独立运行审计；不进行模型推理探测。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Available,
    Missing,
    Unverified,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub requirement: &'static str,
    pub status: Status,
    pub detail: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub backend: &'static str,
    pub backend_ready: bool,
    pub checks: Vec<Check>,
}

/// 生成 schema 并检查受控进程策略与原生认证状态，不创建会话或发起推理。
pub async fn inspect(program: &Path) -> Result<Report, &'static str> {
    inspect_with_timeout(program, Duration::from_secs(30)).await
}

async fn inspect_with_timeout(program: &Path, timeout: Duration) -> Result<Report, &'static str> {
    let directory = generate_schemas(program, timeout).await?;
    let mut report = inspect_schemas(&directory.0)?;
    let confirmed = if controlled_schema(&directory.0)? {
        match crate::codex_rpc::Client::spawn(program).await {
            Ok(mut client) => {
                let confirmed = client
                    .call("llmproxy/capabilities", serde_json::json!({}))
                    .await
                    .is_ok_and(|value| crate::codex_rpc::verify_policy(&value).is_ok());
                client.shutdown().await;
                confirmed
            }
            Err(_) => false,
        }
    } else {
        false
    };
    report.checks.push(interface(
        "controlled_stateless_transport",
        confirmed,
        "进程确认无会话传输、客户端工具策略与原生 ChatGPT 登录；不进行推理探测",
    ));
    report.backend_ready = confirmed;
    if confirmed {
        let authentication = report
            .checks
            .iter_mut()
            .find(|check| check.requirement == "native_authentication")
            .unwrap();
        authentication.status = Status::Available;
        authentication.detail = "Codex 原生登录状态可用；真实账号刷新轮转仍需独立验收";
    }
    Ok(report)
}

pub(crate) async fn require_controlled_transport(program: &Path) -> Result<(), &'static str> {
    let directory = generate_schemas(program, Duration::from_secs(30)).await?;
    if controlled_schema(&directory.0)? {
        Ok(())
    } else {
        Err("Codex 缺少受控无会话传输扩展；未启动 app-server")
    }
}

fn controlled_schema(directory: &Path) -> Result<bool, &'static str> {
    let value = read_schema(&directory.join("llmproxy-proxy-policy.json"))?;
    Ok(value["policyVersion"] == 1
        && value["stateless"] == true
        && value["clientToolsOnly"] == true
        && value["methods"]
            == serde_json::json!([
                "llmproxy/capabilities",
                "llmproxy/responses",
                "llmproxy/cancel"
            ]))
}

async fn generate_schemas(
    program: &Path,
    timeout: Duration,
) -> Result<SchemaDirectory, &'static str> {
    let directory = SchemaDirectory::create()?;
    let mut child = Command::new(program)
        .args([
            "app-server",
            "generate-json-schema",
            "--experimental",
            "--out",
        ])
        .arg(&directory.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "无法启动 Codex schema 生成命令")?;
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(|_| "无法等待 Codex schema 生成命令")?,
        Err(_) => {
            let _ = child.kill().await;
            return Err("Codex schema 生成超时");
        }
    };
    if !status.success() {
        return Err("Codex 未成功生成实验 schema；请检查安装版本");
    }
    Ok(directory)
}

fn inspect_schemas(directory: &Path) -> Result<Report, &'static str> {
    let start = read_schema(&directory.join("v2/ThreadStartParams.json"))?;
    let inject = read_schema(&directory.join("v2/ThreadInjectItemsParams.json"))?;
    let checks = vec![
        interface(
            "ephemeral_thread",
            property_type(&start, "ephemeral", "boolean"),
            "thread/start 支持 ephemeral；不是无正文落盘的运行证明",
        ),
        interface(
            "dynamic_tools",
            property_type(&start, "dynamicTools", "array"),
            "thread/start 支持 dynamicTools；不是禁止全部内置工具的证明",
        ),
        interface(
            "raw_history_injection",
            property_type(&inject, "items", "array")
                && property_type(&inject, "threadId", "string"),
            "thread/inject_items 接受原始历史；工具往返仍需运行验收",
        ),
        Check {
            requirement: "native_authentication",
            status: Status::Unverified,
            detail: "未读取凭据；需要验证 Codex 原生登录及刷新",
        },
        Check {
            requirement: "no_conversation_persistence",
            status: Status::Unverified,
            detail: "需要验证正常、错误、取消及崩溃后的全部正文存储路径",
        },
        Check {
            requirement: "no_builtin_tool_execution",
            status: Status::Unverified,
            detail: "需要证明全部内置工具在执行前被禁止；schema 存在不是执行边界",
        },
        Check {
            requirement: "stateless_tool_roundtrip",
            status: Status::Unverified,
            detail: "需要验证工具交付后释放 turn、原始历史恢复及多工具 ID",
        },
    ];
    Ok(Report {
        backend: "codex",
        backend_ready: false,
        checks,
    })
}

fn interface(requirement: &'static str, available: bool, detail: &'static str) -> Check {
    Check {
        requirement,
        status: if available {
            Status::Available
        } else {
            Status::Missing
        },
        detail: if available {
            detail
        } else {
            "未找到符合类型要求的接口定义"
        },
    }
}

fn property_type(schema: &Value, property: &str, kind: &str) -> bool {
    let value = &schema["properties"][property]["type"];
    value.as_str() == Some(kind)
        || value
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(kind)))
}

fn read_schema(path: &Path) -> Result<Value, &'static str> {
    if !path.exists() {
        return Ok(Value::Null);
    }
    let metadata = fs::metadata(path).map_err(|_| "无法读取 Codex schema")?;
    if metadata.len() > 16 * 1024 * 1024 {
        return Err("Codex schema 超过容量上限");
    }
    let bytes = fs::read(path).map_err(|_| "无法读取 Codex schema")?;
    serde_json::from_slice(&bytes).map_err(|_| "Codex schema 不是有效 JSON")
}

struct SchemaDirectory(PathBuf);

impl SchemaDirectory {
    fn create() -> Result<Self, &'static str> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| "无法生成临时目录标识")?;
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("llmproxy-codex-schema-{id}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "无法创建 schema 临时目录")?;
        Ok(Self(path))
    }
}

impl Drop for SchemaDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn experimental_interfaces_do_not_imply_backend_ready() {
        let directory = SchemaDirectory::create().unwrap();
        fs::create_dir(directory.0.join("v2")).unwrap();
        fs::write(
            directory.0.join("v2/ThreadStartParams.json"),
            json!({"properties": {
                "ephemeral": {"type": ["boolean", "null"]},
                "dynamicTools": {"type": ["array", "null"]}
            }})
            .to_string(),
        )
        .unwrap();
        fs::write(
            directory.0.join("v2/ThreadInjectItemsParams.json"),
            json!({"properties": {
                "items": {"type": "array"}, "threadId": {"type": "string"}
            }})
            .to_string(),
        )
        .unwrap();
        let report = inspect_schemas(&directory.0).unwrap();
        assert!(!report.backend_ready);
        assert!(
            report.checks[..3]
                .iter()
                .all(|check| check.status == Status::Available)
        );
        assert!(
            report.checks[3..]
                .iter()
                .all(|check| check.status == Status::Unverified)
        );
        let path = directory.0.clone();
        drop(directory);
        assert!(!path.exists());
    }

    #[test]
    fn absent_and_malformed_schemas_are_not_capabilities() {
        let directory = SchemaDirectory::create().unwrap();
        let report = inspect_schemas(&directory.0).unwrap();
        assert!(
            report.checks[..3]
                .iter()
                .all(|check| check.status == Status::Missing)
        );
        fs::create_dir(directory.0.join("v2")).unwrap();
        fs::write(directory.0.join("v2/ThreadStartParams.json"), b"not json").unwrap();
        assert_eq!(
            inspect_schemas(&directory.0).unwrap_err(),
            "Codex schema 不是有效 JSON"
        );
        assert!(!property_type(
            &json!({"properties":{"ephemeral":{"type":"string"}}}),
            "ephemeral",
            "boolean"
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn original_binary_is_rejected_before_server_start() {
        use std::os::unix::fs::PermissionsExt;
        let directory = SchemaDirectory::create().unwrap();
        let program = directory.0.join("codex");
        let marker = directory.0.join("server-started");
        fs::write(
            &program,
            format!(
                "#!/bin/sh\nif [ \"$2\" = generate-json-schema ]; then exit 0; fi\ntouch '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(require_controlled_transport(&program).await.is_err());
        assert!(!inspect(&program).await.unwrap().backend_ready);
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_failure_and_timeout_do_not_expose_output() {
        use std::os::unix::fs::PermissionsExt;
        let directory = SchemaDirectory::create().unwrap();
        let program = directory.0.join("codex");
        fs::write(
            &program,
            "#!/bin/sh\necho credential-on-stderr >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            inspect(&program).await.unwrap_err(),
            "Codex 未成功生成实验 schema；请检查安装版本"
        );
        fs::write(&program, "#!/bin/sh\nexec sleep 10\n").unwrap();
        assert_eq!(
            inspect_with_timeout(&program, Duration::from_millis(50))
                .await
                .unwrap_err(),
            "Codex schema 生成超时"
        );
        assert!(inspect(&directory.0.join("absent")).await.is_err());
    }
}

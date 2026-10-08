//! 有界 stdio RPC。运行策略必须收到受控 Codex 的明确确认，不能根据版本号猜测。
use crate::Error;
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const LIMIT: usize = 16 * 1024 * 1024;

pub struct Client {
    pub(crate) child: Child,
    pub(crate) writer: ChildStdin,
    pub(crate) reader: BufReader<ChildStdout>,
    queued: VecDeque<(Value, usize)>,
    queued_bytes: usize,
    pub(crate) next_id: u64,
}

impl Client {
    pub async fn spawn(program: &Path) -> Result<Self, Error> {
        let mut command = Command::new(program);
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("RUST_LOG", "off")
            .env("LLMPROXY_CODEX_PROXY_V1", "1")
            .kill_on_drop(true);
        // 不将宿主的遥测出口带入包含原始对话的子进程。
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("OTEL_") {
                command.env_remove(name);
            }
        }
        let mut child = command.spawn().map_err(|_| "无法启动 Codex app-server")?;
        let writer = child.stdin.take().ok_or("Codex stdin 不可用")?;
        let reader = BufReader::new(child.stdout.take().ok_or("Codex stdout 不可用")?);
        let mut client = Self {
            child,
            writer,
            reader,
            queued: VecDeque::new(),
            queued_bytes: 0,
            next_id: 1,
        };
        client.call("initialize",json!({"clientInfo":{"name":"llmproxy-subscription-agent","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        client.send(json!({"method":"initialized"})).await?;
        Ok(client)
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        tokio::time::timeout(Duration::from_secs(30), self.call_inner(method, params))
            .await
            .map_err(|_| "Codex RPC 等待超时")?
    }

    async fn call_inner(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or("Codex RPC ID 耗尽")?;
        self.send(json!({"method":method,"id":id,"params":params}))
            .await?;
        loop {
            let (message, size) = self.read().await?;
            if message.get("method").is_some() {
                if self.queued.len() >= 64 || self.queued_bytes + size > LIMIT {
                    return Err("Codex 事件队列超过容量上限".into());
                }
                self.queued_bytes += size;
                self.queued.push_back((message, size));
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                return Err("Codex RPC 响应 ID 不匹配".into());
            }
            match (message.get("result"), message.get("error")) {
                (Some(result), None) => return Ok(result.clone()),
                (None, Some(_)) => return Err("Codex RPC 请求失败".into()),
                _ => return Err("Codex RPC 响应形状无效".into()),
            }
        }
    }

    pub async fn event(&mut self) -> Result<Value, Error> {
        if let Some((message, size)) = self.queued.pop_front() {
            self.queued_bytes -= size;
            return Ok(message);
        }
        let (message, _) = self.read().await?;
        if message.get("method").is_none() {
            return Err("Codex 返回未关联的 RPC 响应".into());
        }
        Ok(message)
    }

    pub async fn send(&mut self, message: Value) -> Result<(), Error> {
        let mut bytes = serde_json::to_vec(&message)?;
        if bytes.len() > LIMIT {
            return Err("Codex RPC 请求超过容量上限".into());
        }
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(30), async {
            self.writer.write_all(&bytes).await?;
            self.writer.flush().await
        })
        .await
        .map_err(|_| "Codex RPC 写入超时")?
        .map_err(|_| "Codex RPC 写入失败")?;
        Ok(())
    }

    async fn read(&mut self) -> Result<(Value, usize), Error> {
        let mut bytes = Vec::new();
        let read = (&mut self.reader)
            .take((LIMIT + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| "Codex RPC 读取失败")?;
        if read == 0 {
            return Err("Codex RPC 连接已关闭".into());
        }
        if bytes.len() > LIMIT || !bytes.ends_with(b"\n") {
            return Err("Codex RPC 帧过大或截断".into());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Codex RPC JSON 无效")?;
        if !value.is_object() {
            return Err("Codex RPC 帧必须为对象".into());
        }
        Ok((value, bytes.len()))
    }

    pub async fn shutdown(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

/// 原版不提供该扩展；缺失或部分确认均不能启用原生代理。
pub fn verify_policy(response: &Value) -> Result<(), Error> {
    if response.get("policyVersion").and_then(Value::as_u64) != Some(1)
        || ["stateless", "clientToolsOnly", "nativeAuthReady"]
            .iter()
            .any(|name| response.get(*name).and_then(Value::as_bool) != Some(true))
    {
        return Err("Codex 未确认无会话、客户端工具策略或 ChatGPT 登录，拒绝开始推理".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_or_partial_acknowledgement_never_enables_inference() {
        let good = json!({"policyVersion":1,"stateless":true,"clientToolsOnly":true,"nativeAuthReady":true});
        assert!(verify_policy(&good).is_ok());
        assert!(verify_policy(&json!({})).is_err());
        for name in ["stateless", "clientToolsOnly", "nativeAuthReady"] {
            let mut partial = good.clone();
            partial[name] = json!(false);
            assert!(verify_policy(&partial).is_err());
        }
        let mut incompatible = good;
        incompatible["policyVersion"] = json!(2);
        assert!(verify_policy(&incompatible).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_rpc_preserves_events_and_rejects_mismatched_ids_without_echoing_errors() {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("llmproxy-rpc-{}", crate::random_id().unwrap()));
        std::fs::create_dir(&path).unwrap();
        let program = path.join("mock-codex");
        std::fs::write(&program,"#!/bin/sh\nread init\nprintf '%s\\n' '{\"method\":\"notice\",\"params\":{\"keep\":true}}' '{\"id\":1,\"result\":{}}'\nread initialized\nread second\nprintf '%s\\n' '{\"id\":99,\"error\":{\"message\":\"private-credentials\"}}'\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut client = Client::spawn(&program).await.unwrap();
        assert_eq!(client.event().await.unwrap()["params"]["keep"], true);
        let error = client
            .call("account/read", json!({}))
            .await
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "Codex RPC 响应 ID 不匹配");
        client.shutdown().await;
        std::fs::remove_dir_all(path).unwrap();
    }
}

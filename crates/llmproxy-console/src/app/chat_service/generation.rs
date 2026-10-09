//! 每轮生成独立于页面 HTTP 连接；切换、刷新和关闭页面只结束展示订阅。

use llmproxy_core::thinking::Config;
use std::sync::Arc;

use super::ChatService;
use crate::{
    app::{
        AppState,
        chat_sessions::{ChatSession, TurnRequest},
    },
    chat_stream::chat_reply,
};

/// 取得请求快照后启动后台任务；取消信号和最终保存均归属于指定会话。
pub(crate) fn spawn(
    state: &AppState,
    store: llmproxy_store::ProviderStore,
    id: String,
    room: Arc<ChatSession>,
    turn: TurnRequest,
    streaming: bool,
) {
    let sessions = state.chat_sessions.clone();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        history_header(&format!("Bearer {}", state.history_auth)),
    );
    headers.insert(
        "x-llmproxy-group",
        history_header(&store.group_id().to_string()),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .expect("valid internal HTTP client");
    let origin = state.gateway_origin.clone();
    let history_auth = state.history_auth.clone();
    let mut cancellation = room.cancellation();
    tokio::spawn(async move {
        let request = async {
            let protocol = turn.selection.protocol;
            let (alias, protocols, available, support) =
                super::target::resolve(&store, &turn.selection.model_id).await?;
            if !available || !protocols.contains(&protocol) {
                return Err("当前模型或协议已不可用".to_owned());
            }
            let health = super::target::health(&store, &turn.selection.model_id, protocol).await?;
            if health.blocked {
                return Err(format!("{}，请切换模型或重新探测", health.label));
            }
            Config {
                support,
                ..Default::default()
            }
            .check(turn.thinking)
            .map_err(str::to_owned)?;
            room.request_model(&alias);
            chat_reply(
                &client,
                &origin,
                &turn.selection,
                &alias,
                &turn.history,
                streaming,
                turn.thinking,
                turn.record_id
                    .as_deref()
                    .map(|key| (key, history_auth.as_str())),
                |reply| room.update(reply),
            )
            .await
        };
        // 只在显式停止该会话时取消上游，切换页面不会销毁生成 future。
        let result = tokio::select! {
            biased;
            _ = cancellation.wait_for(|stopped| *stopped) => Err("已停止生成".to_owned()),
            result = request => result,
        };
        if let Err(error) = (ChatService {
            store: &store,
            sessions: &sessions,
        })
        .finish(&id, result)
        .await
        {
            room.save_error(&format!("本轮回复尚未保存：{error}"));
        }
    });
}

fn history_header(value: &str) -> reqwest::header::HeaderValue {
    value.parse().expect("server-generated header")
}

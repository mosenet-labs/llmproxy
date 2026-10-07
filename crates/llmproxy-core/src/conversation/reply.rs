//! 可见回复与结构化历史的保存快照；签名只保留在结构化历史中。
use super::Content;
use crate::ir::media::MediaKind;
use serde::{Deserialize, Serialize};
/// 文本正文之外的独立展示块，不保存签名或原始协议扩展。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayPart {
    /// 空标题表示普通正文，非空标题表示工具或附件说明。
    pub title: String,
    /// 可见正文或格式化后的工具参数/结果。
    pub text: String,
    /// 已筛选的媒体载体。
    pub media: Option<DisplayMedia>,
}

/// 已筛选的可展示媒体地址；缺失格式时仅提供下载。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayMedia {
    /// 用于选择图片、音频、视频或下载控件。
    pub kind: MediaKind,
    /// 可公开访问的地址或经过类型筛选的内联数据。
    pub uri: String,
    /// 是否具备明确且可被浏览器预览的 MIME 类型。
    pub preview: bool,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
/// 页面增量展示与持久化共用的回复投影。
pub struct Reply {
    /// 协议结束状态，用于区分正常结束与达到输出上限。
    #[serde(default)]
    pub status: Option<crate::ir::response::Status>,
    /// 显示给用户的回答正文。
    pub content: String,
    /// 可见思考内容。
    pub thinking: String,
    /// 可见思考的摘要。
    pub summary: String,
    /// 经 IR 统一口径的本轮统计，缺失计数不补零。
    pub usage: Option<crate::ir::usage::Usage>,
    /// 工具、媒体和服务端执行内容；展示标签不进入下一轮文本历史。
    pub parts: Vec<DisplayPart>,
    /// 正常结束后才提供可提交的 IR 历史，展示标签不参与生成。
    pub history: Content,
    /// 响应实际报告的模型，不覆盖当前会话选择。
    pub model: Option<String>,
    /// 本轮历史与协议转换的静态提示，不含参数或签名。
    pub warnings: Vec<String>,
}

impl Reply {
    /// 优先展示可见思考正文，没有正文时使用摘要。
    pub fn visible_thinking(&self) -> &str {
        if self.thinking.is_empty() {
            &self.summary
        } else {
            &self.thinking
        }
    }
}

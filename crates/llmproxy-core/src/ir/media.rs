//! 可跨协议传递的媒体来源；Provider 文件 ID 和公开 URL 严格区分。
use crate::protocol::{
    chat::request::UserPart, gemini::request::Part, messages::request::ContentBlock,
    responses::request::InputPart,
};
use serde::{Deserialize, Serialize};

/// 媒体的语义类别，独立于协议内容块名称。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    /// 静态图片。
    Image,
    /// 音频输入或输出。
    Audio,
    /// 视频内容。
    Video,
    /// 文档或其他文件。
    File,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MediaSource {
    /// 公开地址；转换层不主动下载或上传。
    Url {
        /// 完整资源地址。
        uri: String,
        /// 来源明确给出的 MIME 类型。
        mime_type: Option<String>,
    },
    /// Base64 数据和其 MIME 类型。
    Base64 {
        /// 不含 Data URL 头部的 Base64 数据。
        data: String,
        /// 缺失时不能推测格式。
        mime_type: Option<String>,
    },
    /// 已上传到特定 Provider 的文件，不可直接转交其他协议。
    FileId(String),
    /// 文档中的纯文本数据。
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    /// 目标内容块的语义类别。
    pub kind: MediaKind,
    /// 可移植地址、内联数据或私有资源引用。
    pub source: MediaSource,
    /// 原始文件名或展示名称。
    pub name: Option<String>,
    /// 来源指定的解析精度，例如 low、high 或 auto。
    pub detail: Option<String>,
    /// 只用于同协议未编辑内容的往返；跨协议构造不依赖此副本。
    pub(crate) original: Option<Box<OriginalMedia>>,
}

/// 媒体叶子本身的类型化副本，不保存完整消息或请求 JSON。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OriginalMedia {
    Chat(UserPart),
    Responses(InputPart),
    Messages(ContentBlock),
    Gemini(Box<Part>),
}

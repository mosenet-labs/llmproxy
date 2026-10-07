use topcoat::{
    Result,
    asset::{AssetConfig, Manifest, ManifestEntry},
    router::{Body, response::Response, route},
};
use topcoat_ant_design::{STYLESHEET, STYLESHEET_SHA256};

pub fn config() -> Result<AssetConfig> {
    let mut manifest = Manifest::parse(include_str!(concat!(
        env!("OUT_DIR"),
        "/runtime-manifest.toml"
    )))?;
    manifest.assets.push(ManifestEntry {
        id: STYLESHEET.id(),
        file: "components.css".to_owned(),
        hash: STYLESHEET_SHA256.trim().to_owned(),
        content_type: "text/css".to_owned(),
    });
    Ok(AssetConfig::hosted_at("/ui/assets", manifest))
}

#[route(GET "/ui/assets/components.css")]
pub async fn component_css() -> Result<Response> {
    asset(
        "text/css; charset=utf-8",
        topcoat_ant_design::embedded_stylesheet(),
    )
}

#[route(GET "/ui/assets/console.css")]
pub async fn console_css() -> Result<Response> {
    asset(
        "text/css; charset=utf-8",
        include_str!(concat!(env!("OUT_DIR"), "/console.css")),
    )
}

#[route(GET "/ui/assets/runtime.js")]
pub async fn runtime_js() -> Result<Response> {
    asset(
        "text/javascript; charset=utf-8",
        include_str!(concat!(env!("OUT_DIR"), "/topcoat-runtime.js")),
    )
}

/// 首屏事件绑定完成后恢复生成中的 HTTP shard；不重新调用 Provider。
#[route(GET "/ui/assets/chat-resume.js")]
pub async fn chat_resume_js() -> Result<Response> {
    asset(
        "text/javascript; charset=utf-8",
        r#"const resume = () => requestAnimationFrame(() => {
    const room = document.querySelector('.chat-workspace[data-chat-resume="true"]');
    if (room) room.dispatchEvent(new Event('chatresume'));
});
if (document.readyState === 'complete') resume();
else window.addEventListener('load', resume, { once: true });"#,
    )
}

fn asset(content_type: &'static str, body: &'static str) -> Result<Response> {
    Ok(Response::builder()
        .header("content-type", content_type)
        .header("cache-control", "no-cache")
        .body(Body::from(body))?)
}

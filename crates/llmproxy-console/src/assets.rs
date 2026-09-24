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
    Ok(AssetConfig::hosted_at("/assets", manifest))
}

#[route(GET "/assets/components.css")]
pub async fn component_css() -> Result<Response> {
    asset(
        "text/css; charset=utf-8",
        topcoat_ant_design::embedded_stylesheet(),
    )
}

#[route(GET "/assets/console.css")]
pub async fn console_css() -> Result<Response> {
    asset("text/css; charset=utf-8", include_str!("console.css"))
}

#[route(GET "/assets/runtime.js")]
pub async fn runtime_js() -> Result<Response> {
    asset(
        "text/javascript; charset=utf-8",
        include_str!(concat!(env!("OUT_DIR"), "/topcoat-runtime.js")),
    )
}

fn asset(content_type: &'static str, body: &'static str) -> Result<Response> {
    Ok(Response::builder()
        .header("content-type", content_type)
        .header("cache-control", "no-cache")
        .body(Body::from(body))?)
}

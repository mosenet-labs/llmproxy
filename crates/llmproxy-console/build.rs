use std::{env, fs, path::PathBuf};

use sha2::{Digest, Sha256};
use topcoat_asset::{MANIFEST_VERSION, Manifest, ManifestEntry, RawAsset};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    topcoat::tailwind::BuildConfig::new()
        .input("styles.css")
        .output(out.join("console.css"))
        .render()
        .expect("render console Tailwind stylesheet");

    let id = topcoat_runtime::SCRIPT.id();
    let binary =
        fs::read(env::current_exe().expect("build script path")).expect("read build script");
    let runtime = RawAsset::find_in_binary(&binary)
        .into_iter()
        .find(|asset| asset.id() == id)
        .expect("Topcoat runtime asset");
    let source = runtime.resolved_path();
    let script = fs::read_to_string(&source).expect("read Topcoat runtime script");
    for endpoint in ["procedures", "shards", "pages"] {
        assert!(
            script.contains(&format!("/_topcoat/runtime/{endpoint}")),
            "Topcoat runtime endpoint changed: {endpoint}"
        );
    }
    let script = script.replace("/_topcoat/", "/ui/_topcoat/");
    fs::write(out.join("topcoat-runtime.js"), &script).expect("embed runtime");
    Manifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestEntry {
            id,
            file: "runtime.js".to_owned(),
            hash: Sha256::digest(script.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            content_type: "text/javascript".to_owned(),
        }],
    }
    .save(out.join("runtime-manifest.toml"))
    .expect("write runtime manifest");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=styles.css");
    println!("cargo:rerun-if-changed=src");
}

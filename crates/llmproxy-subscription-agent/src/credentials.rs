//! 凭据与节点身份可落盘；此模块不接受对话数据。
use crate::{Error, random_id};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub struct StateDirectory(pub PathBuf);

impl StateDirectory {
    pub fn open(path: PathBuf) -> Result<Self, Error> {
        if !path.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&path)?;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("状态目录必须是实际目录".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o077 != 0 {
                return Err("状态目录权限必须为 0700".into());
            }
        }
        Ok(Self(path))
    }

    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, Error> {
        let path = self.0.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024
        {
            return Err("凭据文件无效".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o077 != 0 {
                return Err("凭据文件权限必须为 0600".into());
            }
        }
        serde_json::from_slice(&fs::read(path)?)
            .map(Some)
            .map_err(|_| "无法解析凭据文件".into())
    }

    pub fn write<T: Serialize>(&self, name: &str, data: &T) -> Result<(), Error> {
        let temporary = self.0.join(format!(".{}", random_id()?));
        let result = (|| {
            let mut file = private_file(&temporary, true)?;
            serde_json::to_writer(&mut file, data).map_err(|_| "无法序列化凭据")?;
            file.flush()?;
            file.sync_all()?;
            fs::rename(&temporary, self.0.join(name))?;
            File::open(&self.0)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// 同一状态目录只能由一个运行／登录进程持有，防止刷新令牌竞态。
    pub fn lock(&self) -> Result<File, Error> {
        let path = self.0.join("session.lock");
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("锁文件不能是符号链接".into());
        }
        let file = private_file(&path, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if file.metadata()?.mode() & 0o077 != 0 {
                return Err("锁文件权限必须为 0600".into());
            }
        }
        file.try_lock()
            .map_err(|_| "该状态目录已被其他代理或登录进程使用")?;
        Ok(file)
    }
}

fn private_file(path: &Path, exclusive: bool) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create(!exclusive).create_new(exclusive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn private_atomic_credentials_and_exclusive_lock() {
        let path =
            std::env::temp_dir().join(format!("llmproxy-credentials-{}", random_id().unwrap()));
        let directory = StateDirectory::open(path.clone()).unwrap();
        let lock = directory.lock().unwrap();
        assert!(directory.lock().is_err());
        directory
            .write("oauth.json", &serde_json::json!({"token":"test"}))
            .unwrap();
        assert_eq!(
            std::fs::metadata(path.join("oauth.json")).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(
            directory
                .read::<serde_json::Value>("oauth.json")
                .unwrap()
                .unwrap()["token"],
            "test"
        );
        std::fs::set_permissions(
            path.join("oauth.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(directory.read::<serde_json::Value>("oauth.json").is_err());
        drop(lock);
        assert!(directory.lock().is_ok());
        std::fs::remove_dir_all(path).unwrap();
    }
}

use crate::{MedmeError, Vault};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

pub fn object_relpath(hash: &str) -> String {
    format!("objects/{}/{}/{}", &hash[0..2], &hash[2..4], hash)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

impl Vault {
    /// 原子写入 CAS。返回 (hash, 相对路径, 是否实际写入)。已存在则不覆盖。
    pub fn store_object(&self, bytes: &[u8]) -> Result<(String, String, bool), MedmeError> {
        let hash = sha256_hex(bytes);
        let rel = object_relpath(&hash);
        let abs = self.root().join(&rel);
        if abs.exists() {
            return Ok((hash, rel, false));
        }
        let parent = abs.parent().ok_or_else(|| {
            MedmeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "CAS object path has no parent directory",
            ))
        })?;
        std::fs::create_dir_all(parent)?;
        // 唯一临时文件（同目录）+ 原子 persist,避免并发写入共享同一临时文件名
        let mut tmp = NamedTempFile::new_in(parent)?;
        tmp.write_all(bytes)?;
        // Durability: fsync the object's bytes to disk BEFORE the atomic rename,
        // so a crash can never leave a persisted-but-empty/partial CAS object
        // that a later read would trust as valid content.
        tmp.as_file().sync_all()?;
        tmp.persist(&abs).map_err(|e| MedmeError::Io(e.error))?;
        // Durability: fsync the parent directory so the rename itself survives
        // power loss (the entry must be durable, not just the file's bytes).
        std::fs::File::open(parent)?.sync_all()?;
        Ok((hash, rel, true))
    }

    /// vault 根目录的绝对路径 —— 用于展示给用户(如设置页“数据保险箱位置”)、
    /// 或引导其把该目录放进 iCloud/坚果云等云同步目录以实现多设备同步。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 把 vault 内相对路径(如 `storage_path`)拼到 vault root,得到绝对路径。
    pub fn root_join(&self, rel: &str) -> std::path::PathBuf {
        self.root().join(rel)
    }
}

#[cfg(test)]
mod tests {
    use crate::Vault;

    #[test]
    fn store_is_dedup_and_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let (h1, path1, w1) = v.store_object(b"hello medme").unwrap();
        assert!(w1, "first write should persist");
        assert!(dir.path().join(&path1).is_file());

        // 相同内容再存:去重,不重复写
        let (h2, path2, w2) = v.store_object(b"hello medme").unwrap();
        assert_eq!(h1, h2);
        assert_eq!(path1, path2);
        assert!(!w2, "second identical write should be skipped");

        // objects/ 下应只有 1 个对象文件
        let count = walk_files(&dir.path().join("objects"));
        assert_eq!(count, 1);
    }

    fn walk_files(p: &std::path::Path) -> usize {
        let mut n = 0;
        for e in std::fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                n += walk_files(&e.path());
            } else {
                n += 1;
            }
        }
        n
    }
}

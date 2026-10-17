//! 归档解包(zip/tar,含自写的 strip 前缀处理)。
//!
//! 从 `server_manager/mod.rs` 抽出(G8b)。模块间不用 glob(`use super::*`),
//! 而是按编译器提示显式 `use super::<mod>::{…}` —— 双向 glob 会构成 glob 环,
//! 解析器会把环当成空集合。

use std::path::Path;

use anyhow::{anyhow, Context as _, Result};

/// 解压到 out_dir(入口;archive 可为 tar/tar.gz/zip 或单文件 gzip):
/// 单文件 gzip 需给 single_name(解码后写入 out/single_name 并加可执行位)。
#[cfg(test)] // 只有测试用(clippy --lib 看不见测试目标,所以它会报 never used)
pub(crate) fn extract_archive(path: &Path, out_dir: &Path, strip: usize) -> Result<()> {
    let raw = std::fs::read(path)?;
    extract_bytes(&raw, out_dir, strip, None)
}

/// install 用载荷落盘:zip/tar 走目录解压;单文件 gzip(rust-analyzer 类 asset)解码为 bin_rel。
pub(crate) fn install_payload(
    archive: &Path,
    out_dir: &Path,
    strip: usize,
    bin_rel: &str,
) -> Result<()> {
    let raw = std::fs::read(archive)?;
    extract_bytes(&raw, out_dir, strip, Some(bin_rel))
}

pub(crate) fn extract_bytes(
    raw: &[u8],
    out_dir: &Path,
    strip: usize,
    single_name: Option<&str>,
) -> Result<()> {
    use std::io::Read as _;
    std::fs::create_dir_all(out_dir)?;
    if raw.starts_with(&[0x1f, 0x8b]) {
        // gzip:先整解再判 tar(单文件 gzip 也是合法 gzip)
        let mut dec = Vec::new();
        flate2::read::GzDecoder::new(std::io::Cursor::new(raw))
            .read_to_end(&mut dec)
            .with_context(|| "gzip 解压失败")?;
        if is_tar_bytes(&dec) {
            extract_tar_bytes(&dec, out_dir, strip)
        } else {
            let Some(name) = single_name else {
                return Err(anyhow!("gzip 内容不是 tar,且未给单文件目标名"));
            };
            let dest = out_dir.join(name);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&dest, &dec)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(())
        }
    } else if raw.starts_with(b"PK") {
        extract_zip_bytes(raw, out_dir, strip)
    } else {
        extract_tar_bytes(raw, out_dir, strip)
    }
}

pub(crate) fn is_tar_bytes(b: &[u8]) -> bool {
    b.len() > 265 && &b[257..262] == b"ustar"
}

pub(crate) fn extract_zip_bytes(raw: &[u8], out_dir: &Path, strip: usize) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(raw))?;
    let names: Vec<(String, bool)> = (0..z.len())
        .map(|i| {
            let f = z.by_index(i).unwrap();
            (f.name().to_string(), f.is_dir())
        })
        .collect();
    for (name, is_dir) in names {
        let rel = strip_path(&name, strip);
        if rel.is_empty() {
            continue;
        }
        let dest = out_dir.join(&rel);
        if is_dir {
            std::fs::create_dir_all(&dest)?;
        } else {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            let mut src = z.by_name(&name)?;
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut src, &mut out)?;
        }
    }
    Ok(())
}

pub(crate) fn extract_tar_bytes(raw: &[u8], out_dir: &Path, strip: usize) -> Result<()> {
    let mut ar = tar::Archive::new(std::io::Cursor::new(raw));
    for entry in ar.entries()? {
        let mut e = entry?;
        let path_in = e.path()?.into_owned();
        let rel = strip_path(&path_in.to_string_lossy(), strip);
        if rel.is_empty() {
            continue;
        }
        let dest = out_dir.join(&rel);
        if e.header().entry_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            e.unpack(&dest)?;
        }
    }
    Ok(())
}

pub(crate) fn strip_path(name: &str, strip: usize) -> String {
    let mut parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= strip {
        return String::new();
    }
    parts.drain(..strip);
    parts.join("/")
}

use std::process::Command;

/// 解析可用的 ffmpeg 可执行文件路径。
///
/// 优先级：
/// 1. 随包捆绑的 ffmpeg（与 exe 同级的 `ffmpeg/ffmpeg.exe`，Windows 安装包资源）
/// 2. 兜底：`resources/ffmpeg/ffmpeg.exe`（Tauri v2 资源目录变体）
/// 3. 系统 PATH 上的 `ffmpeg`
///
/// 这样即使目标机器没有安装 ffmpeg，也能开箱即用（满足「点开即用的 exe」需求）。
pub fn ffmpeg_executable() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for cand in [
                "ffmpeg/ffmpeg.exe",
                "ffmpeg.exe",
                "resources/ffmpeg/ffmpeg.exe",
                "ffmpeg/ffmpeg",
                "ffmpeg",
            ] {
                let p = parent.join(cand);
                if p.exists() {
                    return p.to_string_lossy().into();
                }
            }
        }
    }
    // 探测 PATH 上的 ffmpeg 是否可用
    for name in ["ffmpeg", "ffmpeg.exe", "avconv"] {
        if Command::new(name)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return name.to_string();
        }
    }
    "ffmpeg".into()
}

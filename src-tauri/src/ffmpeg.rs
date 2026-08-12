use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 解析可用的 ffmpeg 可执行文件路径。
///
/// 优先级：
/// 1. 随包捆绑的 ffmpeg（与 exe 同级的 `ffmpeg/ffmpeg.exe`，Windows 安装包资源）
/// 2. 兜底：`resources/ffmpeg/ffmpeg.exe`（Tauri v2 资源目录变体）
/// 3. 系统 PATH 上的 `ffmpeg`
///
/// 关键：每个候选都要过 `-version` 冒烟测试！
/// 之前只验证 PATH 的候选，捆绑的只要存在就直接用 —— 如果捆绑的 ffmpeg.exe
/// 被损坏（解压失败/被杀毒破坏/隔离残留），spawn 后立即异常退出（0xABAFB008），
/// 且永远不会降级到系统里其他正常 ffmpeg。现在所有候选一律验证，坏的自动跳过。
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
                if p.exists() && ffmpeg_smoke_test(p.to_string_lossy().as_ref()) {
                    return p.to_string_lossy().into();
                }
            }
        }
    }
    // 探测 PATH 上的 ffmpeg 是否可用（隐藏窗口，不闪黑框）
    for name in ["ffmpeg", "ffmpeg.exe", "avconv"] {
        if ffmpeg_smoke_test(name) {
            return name.to_string();
        }
    }
    "ffmpeg".into()
}

/// ffmpeg -version 冒烟测试：能正常输出版本即视为可用
/// 返回 false 说明这个 ffmpeg 跑不起来（损坏 / 缺依赖 / 被杀毒破坏）
fn ffmpeg_smoke_test(bin: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    { cmd.creation_flags(0x08000000); }
    cmd.status()
        .map(|s| s.success())
        .unwrap_or(false)
}

use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::ffmpeg::ffmpeg_executable;

/// 运行 ffmpeg，捕获 stderr 便于错误诊断
fn run_ffmpeg(args: &[String]) -> Result<()> {
    let mut cmd = Command::new(ffmpeg_executable());
    cmd.args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    { cmd.creation_flags(0x08000000); }
    let output = cmd
        .output()
        .map_err(|e| anyhow!("找不到 ffmpeg（{}），请确认已安装并加入 PATH", e))?;
    if !output.status.success() {
        // 优先用 GBK 解码（中文 Windows 默认），失败再回退 UTF-8 容错
        let stderr_bytes = output.stderr;
        let stderr = encoding_rs::GBK
            .decode(&stderr_bytes)
            .0
            .into_owned();
        let stderr = if stderr.contains('\u{FFFD}') {
            String::from_utf8_lossy(&stderr_bytes).into_owned()
        } else {
            stderr
        };
        // 截断到 ~1.5KB 避免错误信息过大
        let tail: String = stderr.chars().rev().take(1500).collect::<String>().chars().rev().collect();
        return Err(anyhow!(
            "ffmpeg 执行失败。参数: {:?}\n\nffmpeg stderr 尾部:\n{}",
            args, tail
        ));
    }
    Ok(())
}

/// 将多个分片按 concat demuxer 合并为单个文件（流式拷贝，极快）
pub fn merge_segments(segments: &[PathBuf], output: &Path) -> Result<()> {
    if segments.is_empty() {
        return Err(anyhow!("没有可合并的分片"));
    }
    let list_path = output.with_extension("concat.txt");
    let mut list = String::new();
    for s in segments {
        let p = fs::canonicalize(s).unwrap_or_else(|_| s.clone());
        let line = p.to_string_lossy().replace('\\', "/");
        list.push_str(&format!("file '{}'\n", line));
    }
    fs::write(&list_path, list)?;
    let args = vec![
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list_path.to_string_lossy().into(),
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().into(),
    ];
    let r = run_ffmpeg(&args);
    let _ = fs::remove_file(&list_path);
    r
}

/// 转码到目标格式
///
/// mp4/mkv/mov 输出 H.264 Baseline + yuv420p + AAC + faststart：
/// 极速编码（ultrafast preset），且 100% WebView2 / 浏览器兼容，
/// 解决抖音原始流编码（可能含 High Profile / AnnexB 等）在浏览器黑屏的问题。
pub fn transcode(input: &Path, output: &Path, format: &str) -> Result<()> {
    let mut args = vec!["-y".into(), "-i".into(), input.to_string_lossy().into()];
    match format {
        "webm" => {
            args.push("-c:v".into());
            args.push("libvpx-vp9".into());
            args.push("-c:a".into());
            args.push("libopus".into());
            args.push("-b:v".into());
            args.push("0".into());
        }
        _ => {
            // 重新编码为 Web 通用兼容的 mp4/mkv/mov 容器
            args.push("-c:v".into());
            args.push("libx264".into());
            args.push("-profile:v".into());
            args.push("baseline".into());
            args.push("-level".into());
            args.push("3.0".into());
            args.push("-pix_fmt".into());
            args.push("yuv420p".into());
            args.push("-preset".into());
            args.push("ultrafast".into());
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("128k".into());
            // mp4/mov 需要 faststart 保证浏览器/WebView2 立即解码
            if format == "mp4" || format == "mov" {
                args.push("-movflags".into());
                args.push("+faststart".into());
            }
        }
    }
    args.push(output.to_string_lossy().into());
    run_ffmpeg(&args)
}

/// 合并多个已存在的视频为单个文件
pub fn merge_videos(inputs: &[PathBuf], output: &Path) -> Result<()> {
    merge_segments(inputs, output)
}

/// 从输入视频中截取多个片段并按顺序合并导出（中间部分自动去除）
pub fn export_segments(input: &Path, segments: &[(f64, f64)], output: &Path) -> Result<()> {
    if segments.is_empty() {
        return Err(anyhow!("没有选取片段"));
    }
    let n = segments.len();
    let mut filter = String::new();
    let mut vlabels = Vec::new();
    let mut alabels = Vec::new();
    for (i, (s, e)) in segments.iter().enumerate() {
        let vi = format!("v{}", i);
        let ai = format!("a{}", i);
        filter.push_str(&format!(
            "[0:v]trim=start={}:end={},setpts=PTS-STARTPTS[{}];",
            s, e, vi
        ));
        filter.push_str(&format!(
            "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[{}];",
            s, e, ai
        ));
        // 注意：concat 语法要求输入是 "[v0][v1]..." 而不是 "v0v1..."
        vlabels.push(format!("[{}]", vi));
        alabels.push(format!("[{}]", ai));
    }
    let vconcat = format!("{}concat=n={}:v=1:a=0[v];", vlabels.join(""), n);
    let aconcat = format!("{}concat=n={}:v=0:a=1[a]", alabels.join(""), n);
    filter.push_str(&vconcat);
    filter.push_str(&aconcat);

    let args = vec![
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into(),
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[v]".into(),
        "-map".into(),
        "[a]".into(),
        output.to_string_lossy().into(),
    ];
    run_ffmpeg(&args)
}

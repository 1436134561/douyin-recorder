use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::ffmpeg::ffmpeg_executable;

/// 运行 ffmpeg，吞掉 stderr（仅用于流水线，不向用户暴露日志）
fn run_ffmpeg(args: &[String]) -> Result<()> {
    let mut cmd = Command::new(ffmpeg_executable());
    cmd.args(args)
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    { cmd.creation_flags(0x08000000); }
    let status = cmd
        .status()
        .map_err(|e| anyhow!("找不到 ffmpeg（{}），请确认已安装并加入 PATH", e))?;
    if !status.success() {
        return Err(anyhow!("ffmpeg 执行失败，参数: {:?}", args));
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
/// mp4/mkv/mov 走 `-c copy` 流拷贝，但会加 `-movflags +faststart`
/// 以保证 WebView2 / 浏览器能立即解码（避免 moov atom 位置问题导致黑屏）。
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
            args.push("-c".into());
            args.push("copy".into());
            // mp4/mov 需要 faststart 以保证浏览器/WebView2 可即时播放
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
        vlabels.push(vi);
        alabels.push(ai);
    }
    let vconcat = format!("{}concat=n={}:v=1:a=0[v]", vlabels.join(""), n);
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

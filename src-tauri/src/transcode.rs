use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::ffmpeg::ffmpeg_executable;

/// 运行 ffmpeg，捕获 stderr（失败时把尾部塞进错误信息，方便定位）
fn run_ffmpeg(args: &[String]) -> Result<()> {
    let mut cmd = Command::new(ffmpeg_executable());
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    { cmd.creation_flags(0x08000000); }
    let out = cmd
        .output()
        .map_err(|e| anyhow!("找不到 ffmpeg（{}），请确认已安装并加入 PATH", e))?;
    if !out.status.success() {
        let raw = String::from_utf8_lossy(&out.stderr).into_owned();
        let n = raw.chars().count();
        let tail: String = if n > 600 {
            raw.chars().skip(n - 600).collect()
        } else {
            raw
        };
        return Err(anyhow!(
            "ffmpeg 执行失败（exit code: {:?}）\n参数: {:?}\n错误尾部:\n{}",
            out.status.code(),
            args,
            tail
        ));
    }
    Ok(())
}

/// 写 concat 清单文件
fn write_concat_list(parts: &[PathBuf], list_path: &Path) -> Result<()> {
    let mut list = String::new();
    for s in parts {
        let p = fs::canonicalize(s).unwrap_or_else(|_| s.clone());
        let line = p.to_string_lossy().replace('\\', "/");
        list.push_str(&format!("file '{}'\n", line));
    }
    fs::write(list_path, list)?;
    Ok(())
}

/// 将多个分片按 concat demuxer 合并为单个文件（同参数流式拷贝，极快）
pub fn merge_segments(segments: &[PathBuf], output: &Path) -> Result<()> {
    if segments.is_empty() {
        return Err(anyhow!("没有可合并的分片"));
    }
    let list_path = output.with_extension("concat.txt");
    write_concat_list(segments, &list_path)?;

    let ext = output
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list_path.to_string_lossy().into_owned(),
    ];
    if ext == "mp4" || ext == "mov" {
        // faststart：WebView2 / 浏览器可以边下边播（moov 前置）
        args.extend(["-c".into(), "copy".into(), "-movflags".into(), "+faststart".into()]);
    } else if ext == "webm" {
        // webm 不支持 H.264，必须重编码
        args.extend([
            "-c:v".into(),
            "libvpx-vp9".into(),
            "-c:a".into(),
            "libopus".into(),
        ]);
    } else {
        args.extend(["-c".into(), "copy".into()]);
    }
    args.push(output.to_string_lossy().into_owned());

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
    let mut args = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
    ];
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
    args.push(output.to_string_lossy().into_owned());
    run_ffmpeg(&args)
}

/// 合并多个已存在的视频为单个文件
pub fn merge_videos(inputs: &[PathBuf], output: &Path) -> Result<()> {
    merge_segments(inputs, output)
}

/// 构建「截取一段」的 ffmpeg 参数
///
/// 关键：`-ss` 放在 `-i` **之前**（input seek，秒级定位），
/// 旧实现用 `-filter_complex trim` 会把整段前置内容解码一遍 ——
/// 对 2 小时的源剪 58:20 那段，等于先解码 58 分钟，慢到不可用。
fn build_cut_args(input: &Path, start: f64, dur: f64, part: &Path, with_audio: bool) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-ss".into(),
        format!("{:.3}", start.max(0.0)),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-t".into(),
        format!("{:.3}", dur.max(0.05)),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
    ];
    if with_audio {
        a.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "128k".into(),
        ]);
    } else {
        a.push("-an".into());
    }
    a.push(part.to_string_lossy().into_owned());
    a
}

/// 从输入视频中截取多个片段并按顺序合并导出（中间部分自动去除）
///
/// 流程：逐段快速定位截取（重编码到统一参数）→ concat 拼接。
/// 相比旧的 trim+concat 滤镜，长视频定位速度从「分钟级」降到「秒级」。
pub fn export_segments(input: &Path, segments: &[(f64, f64)], output: &Path) -> Result<()> {
    if segments.is_empty() {
        return Err(anyhow!("没有选取片段"));
    }
    if !input.exists() {
        return Err(anyhow!("源文件不存在: {}", input.display()));
    }

    let tmp = output.with_file_name(format!(".cliptmp_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;

    let result = (|| -> Result<()> {
        let mut parts: Vec<PathBuf> = Vec::new();
        for (i, (s, e)) in segments.iter().enumerate() {
            let start = s.max(0.0);
            let end = *e;
            let dur = (end - start).max(0.05);
            let part = tmp.join(format!("part_{:05}.mp4", i));
            // 先带音轨；源没有音轨时 ffmpeg 会失败 → 再退化成纯视频
            if run_ffmpeg(&build_cut_args(input, start, dur, &part, true)).is_err() {
                run_ffmpeg(&build_cut_args(input, start, dur, &part, false)).map_err(|err| {
                    anyhow!(
                        "截取片段 {}（{:.1}s ~ {:.1}s）失败: {}",
                        i + 1,
                        start,
                        end,
                        err
                    )
                })?;
            }
            if !part.exists() || fs::metadata(&part).map(|m| m.len()).unwrap_or(0) == 0 {
                return Err(anyhow!("片段 {} 截取结果为空，请检查时间段是否超出视频长度", i + 1));
            }
            parts.push(part);
        }

        let ext = output
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        if ext == "webm" {
            // webm 不能直接 copy H.264，转码拼接
            let list_path = tmp.join("concat.txt");
            write_concat_list(&parts, &list_path)?;
            let args: Vec<String> = vec![
                "-y".into(),
                "-v".into(),
                "error".into(),
                "-f".into(),
                "concat".into(),
                "-safe".into(),
                "0".into(),
                "-i".into(),
                list_path.to_string_lossy().into_owned(),
                "-c:v".into(),
                "libvpx-vp9".into(),
                "-c:a".into(),
                "libopus".into(),
                output.to_string_lossy().into_owned(),
            ];
            run_ffmpeg(&args)
        } else {
            merge_segments(&parts, output)
        }
    })();

    let _ = fs::remove_dir_all(&tmp);
    result
}

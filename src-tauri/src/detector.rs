use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use serde_json;
use tauri::{AppHandle, Emitter, Runtime};

use crate::ffmpeg::ffmpeg_executable;
use crate::logic::{Decision, SitStandLogic};
use crate::recorder;
use crate::state::AppState;
use crate::types::DetectionEvent;

/// 检测器句柄：持有 ffmpeg（取流）与 python（分析）两个子进程
pub struct DetectorHandle {
    pub ffmpeg: Option<std::process::Child>,
    pub python: Option<std::process::Child>,
    pub stopped: AtomicBool,
    #[allow(dead_code)]
    pub armed_start: bool,
}

impl DetectorHandle {
    pub fn stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(mut c) = self.ffmpeg.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(mut c) = self.python.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// 解析 detector.py 脚本路径：优先与 exe 同级的 python/，其次开发态 src-tauri/python/
fn detector_script_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let cand = parent.join("python").join("detector.py");
            if cand.exists() {
                return cand;
            }
        }
    }
    let dev = std::path::PathBuf::from("src-tauri/python/detector.py");
    if dev.exists() {
        return dev;
    }
    std::path::PathBuf::from("detector.py")
}

/// 探测可用的 Python 解释器（优先使用随包捆绑的 python/python.exe）
fn python_executable(cfg_py: &Option<String>) -> String {
    if let Some(p) = cfg_py {
        if !p.is_empty() {
            return p.clone();
        }
    }
    // 随包捆绑的嵌入式 Python（与 exe 同级的 python/python.exe）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for cand in ["python/python.exe", "python.exe", "python/python"] {
                let p = parent.join(cand);
                if p.exists() {
                    return p.to_string_lossy().into();
                }
            }
        }
    }
    for name in ["python", "python3", "py"] {
        if Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return name.to_string();
        }
    }
    "python".into()
}

/// 启动检测器：ffmpeg 取低清低帧灰度流 → python stdin → JSON 事件行 → stdout
#[allow(clippy::too_many_arguments)]
pub fn spawn_detector<R: Runtime>(
    room_id: String,
    source: String,
    mode: String,
    sensitivity: f32,
    sit_stop_seconds: u64,
    armed_start: bool,
    py_path: Option<String>,
    app: AppHandle<R>,
    state: Arc<Mutex<AppState>>,
) -> Result<DetectorHandle> {
    let script = detector_script_path();
    let py = python_executable(&py_path);

    // 构建 ffmpeg 输入参数
    let mut ffmpeg_args: Vec<String> = vec!["-y".into()];
    if mode == "screen" {
        ffmpeg_args.extend([
            "-f".into(),
            "gdigrab".into(),
            "-framerate".into(),
            "3".into(),
            "-i".into(),
            source.clone(),
        ]);
    } else {
        ffmpeg_args.extend(["-i".into(), source.clone()]);
    }
    ffmpeg_args.extend([
        "-vf".into(),
        "scale=320:240".into(),
        "-r".into(),
        "3".into(),
        "-pix_fmt".into(),
        "gray".into(),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ]);

    let ffmpeg_bin = ffmpeg_executable();
    let mut ffmpeg = Command::new(&ffmpeg_bin)
        .args(&ffmpeg_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("启动 ffmpeg 检测流失败: {}", e))?;

    let fout = ffmpeg
        .stdout
        .take()
        .ok_or_else(|| anyhow!("无法获取 ffmpeg stdout"))?;

    let mut python = Command::new(&py)
        .arg(&script)
        .arg("--width")
        .arg("320")
        .arg("--height")
        .arg("240")
        .arg("--sensitivity")
        .arg(sensitivity.to_string())
        .stdin(Stdio::from(fout))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("启动检测器失败: {}", e))?;

    let preader = python
        .stdout
        .take()
        .ok_or_else(|| anyhow!("无法获取检测器 stdout"))?;
    let reader = BufReader::new(preader);

    let app2 = app.clone();
    let state2 = state.clone();
    let room2 = room_id.clone();
    thread::spawn(move || {
        let mut logic = SitStandLogic::new(sensitivity, sit_stop_seconds);
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let ev: DetectionEvent = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let decision = logic.update(&ev);
            let _ = app2.emit(
                "detection_event",
                serde_json::json!({
                    "room_id": room2,
                    "state": ev.state,
                    "motion": ev.motion,
                    "conf": ev.conf,
                    "decision": format!("{:?}", decision),
                }),
            );

            let should_start = decision == Decision::Start && armed_start;
            let should_stop = decision == Decision::Stop;
            if should_start || should_stop {
                let recording = state2.lock().unwrap().recordings.contains_key(&room2);
                if should_start && !recording {
                    // 自动开始录制（不再新开检测器，避免递归）
                    let _ = recorder::begin_recording(&room2, false, &app2, &state2);
                } else if should_stop && recording {
                    let _ = recorder::stop_and_finalize(&room2, &app2, &state2);
                    logic.reset();
                }
            }
        }
        // 检测流结束（EOF）：标记停止
        if let Ok(mut st) = state2.lock() {
            if let Some(h) = st.detectors.get_mut(&room2) {
                h.stopped.store(true, Ordering::SeqCst);
            }
        }
    });

    Ok(DetectorHandle {
        ffmpeg: Some(ffmpeg),
        python: Some(python),
        stopped: AtomicBool::new(false),
        armed_start,
    })
}

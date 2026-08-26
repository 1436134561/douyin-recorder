//! 回归测试：验证「开始录制 / 停止录制」按钮背后的后端代码路径
//! （分段录制 → 合并 → 转码）在 ffmpeg 可用时能产生非空输出文件，
//! 并在 ffmpeg 缺失时优雅返回 Err 而非 panic（保证前端能捕获并报错）。
//!
//! 使用 `tauri::test::mock_builder`(MockRuntime) 以避免在无显示环境创建真实窗口。
//! 运行需启用 test feature：`cargo test --features test`

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use tauri::test::{mock_builder, mock_context, noop_assets};

    use crate::config::{AppConfig, RoomConfig};
    use crate::recorder;
    use crate::state::{AppState, SharedState};

    fn ffmpeg_present() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// 检查 ffmpeg 是否支持 H.264 编码（决定是否能跑录屏流水线测试）
    fn ffmpeg_has_libx264() -> bool {
        // 用 ffmpeg 询问 libx264 帮助：若不存在会输出 "Codec 'libx264' not found" 或类似
        let out = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-h", "encoder=libx264"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();
        match out {
            Ok(o) => {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                s.push_str(&String::from_utf8_lossy(&o.stderr));
                // 存在标志：列出 "Encoder libx264" 段
                // 不存在标志："not found" / "Unknown encoder"
                let has_help = s.contains("Encoder libx264");
                let denied = s.contains("not found") || s.contains("Unknown encoder");
                has_help && !denied
            }
            Err(_) => false,
        }
    }

    /// 选一个能编码 H.264 的 ffmpeg 用于生成测试样本（本机 /usr/bin/ffmpeg 含 libx264，
    /// 静态版 /usr/local/bin/ffmpeg 不含；Windows CI 上回退到 PATH 的 ffmpeg）
    fn sample_ffmpeg() -> String {
        if Path::new("/usr/bin/ffmpeg").exists() {
            "/usr/bin/ffmpeg".into()
        } else {
            "ffmpeg".into()
        }
    }

    fn make_sample(path: &Path) {
        let st = std::process::Command::new(sample_ffmpeg())
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=15",
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                &path.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        assert!(
            st.map(|s| s.success()).unwrap_or(false),
            "生成测试样本失败（ffmpeg 不可用？）"
        );
    }

    fn test_state(out_dir: std::path::PathBuf, capture_mode: &str) -> SharedState {
        Arc::new(Mutex::new(AppState {
            config: AppConfig {
                output_dir: out_dir,
                output_format: "mp4".into(),
                auto_mp4: true,
                segment_minutes: 1,
                detect_enabled: false,
                sensitivity: 1.0,
                sit_stop_seconds: 5,
                autostart: false,
                capture_mode: capture_mode.into(),
                screen_source: Some("desktop".into()),
                python_path: None,
                rooms: vec![],
                monitor_poll_secs: 30,
                url_renew_lead_secs: 120,
                max_renew_attempts: 3,
                url_renew_interval_secs: 900,
            },
            recordings: Default::default(),
            detectors: Default::default(),
            logic: Default::default(),
            monitors: Default::default(),
            monitor_states: Default::default(),
        }))
    }

    /// 构造一个 MockRuntime 测试 App，handle 类型与运行时无关（泛型函数可接收）
    fn test_app(shared: SharedState) -> tauri::AppHandle<tauri::test::MockRuntime> {
        let app = mock_builder()
            .manage(shared)
            .build(mock_context(noop_assets()))
            .expect("test app");
        app.handle().clone()
    }

    #[test]
    fn recording_pipeline_produces_merged_output() {
        if !ffmpeg_present() {
            eprintln!("ffmpeg 不可用，跳过录制流水线测试");
            return;
        }
        if !ffmpeg_has_libx264() {
            eprintln!("ffmpeg 不含 libx264，跳过录制流水线测试（H.264 重新编码必需）");
            return;
        }
        let base = std::env::temp_dir().join("douyin_recorder_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let sample = base.join("sample.mp4");
        make_sample(&sample);
        let out_dir = base.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let shared = test_state(out_dir, "auto");
        let handle = test_app(shared.clone());

        {
            let mut st = shared.lock().unwrap();
            let mut room = RoomConfig::new("testroom".into());
            room.stream_url = Some(sample.to_string_lossy().into());
            st.config.rooms.push(room);
        }

        recorder::begin_recording("testroom", false, &handle, &shared)
            .expect("begin_recording 应成功");
        // 等待分片生成（样本 2s，segment 60s → 产生单段）
        std::thread::sleep(std::time::Duration::from_millis(3000));
        let info = recorder::stop_and_finalize("testroom", false, &handle, &shared)
            .expect("stop_and_finalize 应成功");
        assert!(Path::new(&info.path).exists(), "最终文件应存在: {}", info.path);
        let size = std::fs::metadata(&info.path).unwrap().len();
        assert!(size > 0, "最终文件不应为空，实际大小 {}", size);
    }

    #[test]
    fn missing_ffmpeg_returns_error_not_panic() {
        if ffmpeg_present() {
            // 本机有 ffmpeg，无法直接模拟缺失，跳过（不影响按钮路径验证）
            return;
        }
        let base = std::env::temp_dir().join("douyin_recorder_test_noffmpeg");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let shared = test_state(base, "screen");
        let handle = test_app(shared.clone());
        {
            let mut st = shared.lock().unwrap();
            st.config.rooms.push(RoomConfig::new("r".into()));
        }
        // screen 模式依赖 ffmpeg；缺失时应返回 Err（前端可捕获并提示），而非 panic
        let r = recorder::begin_recording("r", false, &handle, &shared);
        assert!(r.is_err(), "ffmpeg 缺失时应返回 Err 而非 panic");
    }

    /// gather_segments 多代收集：legacy 根目录 + gen0 + gen1 顺序正确且排除 merged.flv
    #[test]
    fn gather_segments_multi_generation_order() {
        use std::path::PathBuf;
        let base = std::env::temp_dir().join("douyin_recorder_test_gather");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(base.join("gen0")).unwrap();
        std::fs::create_dir_all(base.join("gen1")).unwrap();
        std::fs::create_dir_all(base.join("gen10")).unwrap(); // 验证数字排序 gen10 在 gen1 后

        // 写假分片（>1KB）
        fn write_flv(p: &PathBuf, size: usize) {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; size]).unwrap();
        }
        write_flv(&base.join("seg_000.flv"), 2048);       // legacy 最早
        write_flv(&base.join("gen0").join("seg_000.flv"), 2048);
        write_flv(&base.join("gen0").join("seg_001.flv"), 2048);
        write_flv(&base.join("gen1").join("seg_000.flv"), 2048);
        write_flv(&base.join("gen10").join("seg_000.flv"), 2048);
        write_flv(&base.join("merged.flv"), 2048);        // 产物，应被排除
        // 空壳（<1KB）应被过滤
        write_flv(&base.join("gen0").join("seg_002.flv"), 512);

        let session = crate::recorder::RecordingSession {
            room_id: "test".into(),
            mode: "stream".into(),
            work_dir: base.clone(),
            process: None,
            started_at: 0,
            renew_attempts: 0,
            current_url: "http://x".into(),
            generation: 0,
            last_renew_at: 0,
            renewing: false,
        };
        let files = crate::recorder::gather_segments(&session);
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&base)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "seg_000.flv",                    // legacy
                "gen0/seg_000.flv",               // gen0 内按文件名排序
                "gen0/seg_001.flv",
                "gen1/seg_000.flv",               // gen1
                "gen10/seg_000.flv",              // gen10（数字排序在 gen1 后）
            ],
            "多代收集顺序应为 legacy → gen0 → gen1 → gen10，且排除 merged.flv 与空壳"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// flv_expiry_secs：绝对秒 / 毫秒级 / 相对秒 / 无参数
    #[test]
    fn flv_expiry_secs_parsing() {
        use crate::stream_resolver::flv_expiry_secs;
        // 绝对秒（10 位）
        assert_eq!(flv_expiry_secs("http://x.flv?expire=1787237249"), Some(1787237249));
        // 毫秒级（13 位）→ ÷1000
        assert_eq!(flv_expiry_secs("http://x.flv?expire=1787237249000"), Some(1787237249));
        // 相对秒（<1e9）→ now + 1800
        let v = flv_expiry_secs("http://x.flv?expire=600").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(v >= now + 1790 && v <= now + 1810, "相对秒应约为 now+1800，实际 {}", v);
        // 无过期参数 → None
        assert_eq!(flv_expiry_secs("http://x.flv?sign=abc"), None);
        // 无查询参数 → None
        assert_eq!(flv_expiry_secs("http://x.flv"), None);
    }
}

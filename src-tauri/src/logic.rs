use std::time::{Duration, Instant};

use crate::types::DetectionEvent;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Decision {
    /// 主播站立/有动作：应保持或开始录制
    Start,
    /// 主播坐下且持续无动作达到阈值：停止录制
    Stop,
    /// 状态未变，维持现状
    Continue,
}

/// 坐立状态机：集成人脸占比 + 动作强度双判据，带防抖
pub struct SitStandLogic {
    sitting_since: Option<Instant>,
    motion_threshold: f32,
    sit_stop: Duration,
}

impl SitStandLogic {
    pub fn new(sensitivity: f32, sit_stop_seconds: u64) -> Self {
        // 灵敏度越高，动作阈值越低（更易触发“在场”）。
        // motion 为变化像素占比(0~1)，小幅动作通常 1%~5%。
        let motion_threshold = (0.012 / sensitivity.max(0.1)).clamp(0.002, 0.2);
        SitStandLogic {
            sitting_since: None,
            motion_threshold,
            sit_stop: Duration::from_secs(sit_stop_seconds.max(1)),
        }
    }

    /// 输入一帧检测结果，返回对录制的决策
    pub fn update(&mut self, ev: &DetectionEvent) -> Decision {
        let motion_high = ev.motion > self.motion_threshold;
        if ev.state == "standing" || motion_high {
            // 站立或有明显动作：恢复录制，清空坐下计时
            self.sitting_since = None;
            Decision::Start
        } else if ev.state == "sitting" {
            match self.sitting_since {
                None => {
                    self.sitting_since = Some(Instant::now());
                    Decision::Continue
                }
                Some(t) => {
                    if t.elapsed() >= self.sit_stop {
                        Decision::Stop
                    } else {
                        Decision::Continue
                    }
                }
            }
        } else {
            // unknown / 无明确姿态：保持当前计时，不重置（避免抖动误停）
            Decision::Continue
        }
    }

    pub fn reset(&mut self) {
        self.sitting_since = None;
    }
}

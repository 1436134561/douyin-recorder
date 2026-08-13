/* DouyinRecorder Web UI 逻辑。通过 window.pywebview.api 与后端通信。 */
(function () {
  "use strict";
  let api = null;
  let lastLogIdx = 0;
  const $ = (s) => document.querySelector(s);
  const $$ = (s) => Array.from(document.querySelectorAll(s));

  function ready(cb) {
    if (window.pywebview && window.pywebview.api) { api = window.pywebview.api; cb(); }
    else if (window.pywebviewready) { /* noop */ }
    else { setTimeout(() => ready(cb), 80); }
  }

  // ---------- 全局错误提示（避免“点了没反应”却无任何反馈）----------
  let _toastTimer = null;
  function toast(msg, type) {
    const el = $("#toast");
    if (!el) return;
    el.textContent = msg;
    el.className = "toast" + (type ? " " + type : "");
    el.hidden = false;
    clearTimeout(_toastTimer);
    _toastTimer = setTimeout(() => { el.hidden = true; }, 3200);
  }
  window.addEventListener("error", (e) => {
    toast("脚本错误：" + (e.message || e.error || "未知"), "err");
    console.error("window error", e);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    toast("运行错误：" + (r && r.message ? r.message : r), "err");
    console.error("unhandledrejection", r);
  });

  async function call(fn, ...args) {
    if (!api) return null;
    try { return await api[fn](...args); }
    catch (e) { console.error(fn, e); toast("调用失败：" + fn, "err"); return null; }
  }

  // ---------- 渲染（增量更新，避免每轮轮询重建卡片导致入场动画重播=抖动）----------
  const cardMap = new Map();   // tid -> 卡片元素

  async function refreshState() {
    const st = await call("get_state");
    if (!st) return;
    const cards = $("#cards");
    $("#emptyHint").style.display = st.tasks.length ? "none" : "block";
    const seen = new Set();
    st.tasks.forEach((t) => {
      seen.add(String(t.id));
      let el = cardMap.get(t.id);
      if (!el) { el = renderCard(t); cardMap.set(t.id, el); cards.appendChild(el); }   // 新建才播动画
      else { updateCard(el, t); }                                                     // 已有则原地更新
    });
    for (const [tid, el] of cardMap) {
      if (!seen.has(String(tid))) { el.remove(); cardMap.delete(tid); }
    }
  }

  function isOff(status) { return status === "离线" || status === "解析失败" || status === "错误"; }
  function stateClass(t) { return t.status === "录制中" ? "is-rec" : (isOff(t.status) ? "is-off" : "is-on"); }
  function statusBadgeCls(t) { return t.status === "录制中" ? "rec" : (isOff(t.status) ? "off" : "on"); }

  // 生成徽章 HTML（状态 + 智能识别 + 解析失败原因）
  // 修复：status 已是「解析失败」时不再用字面“解析失败”追加第二个徽章，
  // 改为展示真实错误原因（截断），避免双徽章重叠。
  function badgesHTML(t) {
    let h = badge(statusBadgeCls(t), t.status);
    if (t.smart_state) h += `<span class="badge smart">${esc(t.smart_state)}</span>`;
    if (t.error) {
      const reason = String(t.error).replace(/\s+/g, " ").trim().slice(0, 46);
      h += `<span class="badge err" title="${esc(t.error)}">${esc(reason)}</span>`;
    }
    return h;
  }

  function renderCard(t) {
    const el = document.createElement("div");
    el.className = "card " + stateClass(t);
    el.innerHTML = `
      <div class="name">${esc(t.name)}</div>
      <div class="url">${esc(t.url)}</div>
      <div class="badges">${badgesHTML(t)}</div>
      <div class="ctrls">
        <button class="btn small" data-act="start">开始</button>
        <button class="btn small" data-act="stop">停止</button>
        <button class="btn small" data-act="preview">预览</button>
        <button class="btn small" data-act="remove">移除</button>
      </div>`;
    el.querySelector('[data-act="start"]').onclick = () => call("start_task", t.id);
    el.querySelector('[data-act="stop"]').onclick = () => call("stop_task", t.id);
    el.querySelector('[data-act="remove"]').onclick = () => call("remove_task", t.id);
    el.querySelector('[data-act="preview"]').onclick = () => openPreview(t.id);
    return el;
  }

  function updateCard(el, t) {
    el.className = "card " + stateClass(t);                 // 仅改修饰类，不重播 cardIn
    el.querySelector(".name").textContent = t.name || "";
    el.querySelector(".url").textContent = t.url || "";
    el.querySelector(".badges").innerHTML = badgesHTML(t);  // 徽章无动画，安全重渲染
  }

  function badge(cls, text) { return `<span class="badge ${cls}">${esc(text)}</span>`; }
  function esc(s) { return String(s == null ? "" : s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }

  async function refreshLogs() {
    const logs = await call("get_logs", lastLogIdx);
    if (!logs) return;
    const body = $("#logBody");
    logs.forEach((e) => {
      const d = document.createElement("div");
      d.textContent = `[${e.t}] ${e.msg}`;
      body.appendChild(d);
    });
    lastLogIdx += logs.length;
    body.scrollTop = body.scrollHeight;
  }

  // ---------- 任务 ----------
  $("#addBtn").onclick = async () => {
    const v = $("#addInput").value.trim();
    if (!v) return;
    await call("add_task", v);
    $("#addInput").value = "";
    refreshState();
  };
  $("#addInput").addEventListener("keydown", (e) => { if (e.key === "Enter") $("#addBtn").click(); });
  $("#startAllBtn").onclick = () => call("start_all");
  $("#stopAllBtn").onclick = () => call("stop_all");
  $("#openFolderBtn").onclick = () => call("open_folder");

  // ---------- 弹窗通用 ----------
  $$("[data-close]").forEach((b) => b.onclick = () => closeModals());
  function closeModals() { $$(".modal").forEach((m) => (m.hidden = true)); }
  $$(".modal").forEach((m) => m.addEventListener("click", (e) => { if (e.target === m) closeModals(); }));

  // ---------- 设置 ----------
  $("#settingsBtn").onclick = async () => {
    let cfg;
    try {
      cfg = await call("get_config");
    } catch (e) {
      toast("读取配置失败：" + (e && e.message ? e.message : e), "err");
      return;
    }
    if (!cfg) { toast("配置为空，请重试", "warn"); return; }
    try {
      setVal("cfg_output_dir", cfg.output_dir);
      setVal("cfg_filename_template", cfg.filename_template);
      setChk("cfg_smart_record", cfg.smart_record);
      setVal("cfg_smart_mode", cfg.smart_mode);
      setVal("cfg_smart_interval", cfg.smart_interval);
      setVal("cfg_smart_motion_threshold", cfg.smart_motion_threshold);
      setVal("cfg_smart_posture_span", cfg.smart_posture_span);
      setVal("cfg_smart_posture_bottom_ratio", cfg.smart_posture_bottom_ratio);
      setVal("cfg_check_offline_interval", cfg.check_offline_interval);
      setChk("cfg_low_resource", cfg.low_resource);
      setChk("cfg_auto_transcode", cfg.auto_transcode);
      setVal("cfg_transcode_format", cfg.transcode_format);
      setVal("cfg_transcode_mode", cfg.transcode_mode);
      setChk("cfg_transcode_delete_src", cfg.transcode_delete_src);
      setVal("cfg_cookie", cfg.cookie);
      setVal("cfg_proxy", cfg.proxy);
      setChk("cfg_startup_auto_launch", cfg.startup_auto_launch);
      $("#settingsModal").hidden = false;
    } catch (e) {
      toast("填充设置失败：" + (e && e.message ? e.message : e), "err");
    }
  };
  $("#pickOutBtn").onclick = async () => {
    const d = await call("pick_dir");
    if (d) setVal("cfg_output_dir", d);
  };
  $("#saveCfgBtn").onclick = async () => {
    const cfg = {
      output_dir: val("cfg_output_dir"), filename_template: val("cfg_filename_template"),
      smart_record: chk("cfg_smart_record"), smart_mode: val("cfg_smart_mode"),
      smart_interval: num("cfg_smart_interval"), smart_motion_threshold: num("cfg_smart_motion_threshold"),
      smart_posture_span: num("cfg_smart_posture_span"), smart_posture_bottom_ratio: num("cfg_smart_posture_bottom_ratio"),
      check_offline_interval: num("cfg_check_offline_interval"), low_resource: chk("cfg_low_resource"),
      auto_transcode: chk("cfg_auto_transcode"), transcode_format: val("cfg_transcode_format"),
      transcode_mode: val("cfg_transcode_mode"), transcode_delete_src: chk("cfg_transcode_delete_src"),
      cookie: val("cfg_cookie"), proxy: val("cfg_proxy"), startup_auto_launch: chk("cfg_startup_auto_launch"),
    };
    const r = await call("save_config", cfg);
    if (r && r.ok) { closeModals(); toast("设置已保存", "ok"); }
    else { toast("保存失败：" + ((r && r.msg) || "未知"), "err"); }
  };

  // ---------- 批量 ----------
  $("#batchBtn").onclick = () => { $("#batchText").value = ""; $("#batchModal").hidden = false; };
  $("#batchLoadBtn").onclick = async () => {
    const files = await call("pick_files");
    if (files && files.length) {
      const r = await call("load_text_file", files[0]);
      if (r && r.ok) $("#batchText").value = r.text;
    }
  };
  $("#batchImportBtn").onclick = async () => {
    const r = await call("add_tasks", $("#batchText").value);
    if (r && r.ok) { closeModals(); refreshState(); }
  };

  // ---------- 合并 ----------
  let mergeFiles = [];
  $("#mergeBtn").onclick = () => { mergeFiles = []; $("#mergeList").innerHTML = ""; $("#mergeMsg").textContent = ""; $("#mergeModal").hidden = false; };
  $("#mergePickBtn").onclick = async () => {
    const fs = await call("pick_files");
    if (fs && fs.length) { mergeFiles = fs; renderFileList("#mergeList", mergeFiles); }
  };
  $("#mergeRunBtn").onclick = async () => {
    const r = await call("merge_files", mergeFiles, val("mergeFmt"), val("mergeMode"), chk("mergeLow"));
    $("#mergeMsg").textContent = r && r.ok ? "合并完成: " + (r.out || "") : "合并失败: " + ((r && r.msg) || "");
    if (r && r.ok) setTimeout(closeModals, 1200);
  };

  // ---------- 已完成录屏 / 剪辑 ----------
  $("#recBtn").onclick = async () => {
    const list = await call("list_recordings") || [];
    const ul = $("#recList"); ul.innerHTML = "";
    list.forEach((f) => {
      const li = document.createElement("li");
      li.innerHTML = `<span>${esc(f.name)}</span><span class="sz">${(f.size/1048576).toFixed(1)} MB · <a href="#" data-open>打开</a> · <a href="#" data-clip>剪辑</a></span>`;
      li.querySelector("[data-open]").onclick = (e) => { e.preventDefault(); call("open_folder", f.path); };
      li.querySelector("[data-clip]").onclick = (e) => { e.preventDefault(); openClip(f); };
      ul.appendChild(li);
    });
    $("#recModal").hidden = false;
  };

  let clipSegs = [];
  function openClip(f) {
    clipSegs = [];
    $("#clipVideo").src = "file:///" + f.path.replace(/\\/g, "/");
    $("#clipSegs").innerHTML = ""; $("#clipMsg").textContent = "";
    $("#clipPath") || (window.__clipPath = f.path);
    window.__clipPath = f.path;
    $("#clipModal").hidden = false;
    const v = $("#clipVideo");
    v.ontimeupdate = () => { $("#clipCur").textContent = "当前 " + v.currentTime.toFixed(1) + "s"; };
  }
  $("#markStartBtn").onclick = () => addSeg("start");
  $("#markEndBtn").onclick = () => addSeg("end");
  function addSeg(kind) {
    const v = $("#clipVideo");
    if (!clipSegs.length || clipSegs[clipSegs.length - 1].end != null) clipSegs.push({ start: v.currentTime, end: null });
    if (kind === "end" && clipSegs[clipSegs.length - 1].end == null) clipSegs[clipSegs.length - 1].end = v.currentTime;
    renderSegs();
  }
  function renderSegs() {
    const ul = $("#clipSegs"); ul.innerHTML = "";
    clipSegs.forEach((s, i) => {
      const li = document.createElement("li");
      li.innerHTML = `<span>片段${i+1}: ${s.start.toFixed(1)}s ~ ${(s.end==null?"…":s.end.toFixed(1))}s</span>`;
      ul.appendChild(li);
    });
  }
  $("#clipExportBtn").onclick = async () => {
    const segs = clipSegs.filter((s) => s.end != null && s.end > s.start);
    if (!segs.length) { $("#clipMsg").textContent = "请先标记完整片段"; return; }
    const out = window.__clipPath.replace(/(\.[^.]+)$/, "_clip$1");
    const r = await call("clip", window.__clipPath, segs, out);
    $("#clipMsg").textContent = r && r.ok ? "导出完成: " + (r.out || "") : "导出失败: " + ((r && r.msg) || "");
  };

  // ---------- 实时预览 ----------
  let previewTimer = null, previewTid = null;
  function openPreview(tid) {
    previewTid = tid;
    $("#previewModal").hidden = false;
    call("start_preview", tid).then(() => {
      previewTimer = setInterval(async () => {
        const b64 = await call("get_preview_frame", tid);
        if (b64) $("#previewImg").src = "data:image/jpeg;base64," + b64;
      }, 800);
    });
  }
  function stopPreview() {
    if (previewTimer) clearInterval(previewTimer);
    previewTimer = null;
    if (previewTid != null) call("stop_preview", previewTid);
    previewTid = null;
  }
  $("#previewModal").addEventListener("click", (e) => { if (e.target.id === "previewModal" || e.target.dataset.close !== undefined) { stopPreview(); closeModals(); } });

  // ---------- 日志抽屉 ----------
  $("#logToggle").onclick = () => $("#logDrawer").classList.toggle("open");
  $("#logClose").onclick = () => $("#logDrawer").classList.remove("open");

  // ---------- 主题 ----------
  $("#themeBtn").onclick = () => {
    const cur = document.documentElement.getAttribute("data-theme");
    document.documentElement.setAttribute("data-theme", cur === "dark" ? "light" : "dark");
  };

  // ---------- 工具 ----------
  function val(id) { return document.getElementById(id).value; }
  function num(id) { return parseFloat(document.getElementById(id).value) || 0; }
  function chk(id) { return document.getElementById(id).checked; }
  function setVal(id, v) { if (v != null) document.getElementById(id).value = v; }
  function setChk(id, v) { document.getElementById(id).checked = !!v; }
  function renderFileList(sel, arr) {
    const ul = $(sel); ul.innerHTML = "";
    arr.forEach((f) => { const li = document.createElement("li"); li.innerHTML = `<span>${esc(f)}</span>`; ul.appendChild(li); });
  }

  // ---------- 启动轮询 ----------
  ready(() => {
    call("version").then((v) => { if (v) $("#ver").textContent = "v" + v; });
    refreshState(); refreshLogs();
    setInterval(refreshState, 600);
    setInterval(refreshLogs, 700);
  });
})();

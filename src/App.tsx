import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, currentMonitor } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

type RecordingFile = { name: string; path: string; size: number; modified: number };
type OrphanRecord = {
  temp_video: string;
  temp_sys: string | null;
  temp_mic: string | null;
  suggested_final: string;
};
type EncoderCaps = { cpu: boolean; nvenc: boolean; amf: boolean; draw_mouse_supported: boolean };

// ==================== TOASTS ====================
type Toast = { id: number; msg: string; kind: "info" | "error" };
let toastId = 0;

function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  useEffect(() => {
    const un = listen<string>("recording-error", (e) => {
      const t = { id: ++toastId, msg: e.payload, kind: "error" as const };
      setToasts((p) => [...p, t]);
      setTimeout(() => setToasts((p) => p.filter((x) => x.id !== t.id)), 6000);
    });
    return () => { un.then((f) => f()); };
  }, []);
  return toasts;
}

function Toasts({ toasts }: { toasts: Toast[] }) {
  return (
    <div className="fixed top-3 right-3 z-[9999] flex flex-col gap-2 pointer-events-none">
      {toasts.map((t) => (
        <div key={t.id}
          className={`px-3 py-2 rounded-lg text-xs shadow-2xl border backdrop-blur-md max-w-xs ${
            t.kind === "error"
              ? "bg-red-950/90 border-red-700 text-red-200"
              : "bg-zinc-900/90 border-zinc-700 text-zinc-200"
          }`}>
          {t.msg}
        </div>
      ))}
    </div>
  );
}

// ==================== ROOT ====================
function App() {
  const [route, setRoute] = useState(window.location.hash);
  const toasts = useToasts();

  useEffect(() => {
    const onHash = () => setRoute(window.location.hash);
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  return (
    <>
      <Toasts toasts={toasts} />
      {route === "#/config" && <ConfigView />}
      {route === "#/gallery" && <GalleryView />}
      {route === "#/timer" && <TimerWidget />}
      {(route === "" || !["#/config", "#/gallery", "#/timer"].includes(route)) && <OverlayRecorderView />}
    </>
  );
}

// ==================== TIMER ====================
function TimerWidget() {
  const [seconds, setSeconds] = useState(0);
  const [meta, setMeta] = useState({ fps: "60", size: "0" });

  useEffect(() => {
    setSeconds(0);
    const un = listen("recording-started", () => setSeconds(0));
    return () => { un.then((f) => f()); };
  }, []);

  useEffect(() => {
    const t = setInterval(() => setSeconds((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    const un = listen<{ key: string; value: string }>("recording-progress", (e) => {
      const { key, value } = e.payload;
      setMeta((m) => {
        if (key === "fps") return { ...m, fps: value };
        if (key === "total_size") return { ...m, size: (parseInt(value) / 1024 / 1024).toFixed(1) };
        return m;
      });
    });
    return () => { un.then((f) => f()); };
  }, []);

  const stop = async () => { try { await invoke("stop_capture"); } catch (e) { console.error(e); } };
  const fmt = (sec: number) => {
    const m = Math.floor(sec / 60).toString().padStart(2, "0");
    const s = (sec % 60).toString().padStart(2, "0");
    return `${m}:${s}`;
  };
  const handleMouseDown = async (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).tagName !== "BUTTON") await getCurrentWindow().startDragging();
  };

  return (
    <div onMouseDown={handleMouseDown}
      className="w-screen h-screen bg-zinc-950/95 border border-zinc-700/80 text-white rounded-xl flex items-center justify-between px-3 select-none shadow-2xl cursor-grab">
      <div className="flex items-center gap-2 pointer-events-none">
        <span className="w-2.5 h-2.5 rounded-full bg-red-500 animate-ping" />
        <div className="flex flex-col leading-tight">
          <span className="font-mono text-xs font-bold text-red-400">{fmt(seconds)}</span>
          <span className="text-[9px] font-mono text-zinc-500">{meta.fps} fps · {meta.size} MB</span>
        </div>
      </div>
      <button onClick={stop}
        className="bg-red-600 hover:bg-red-500 text-white text-[10px] px-2.5 py-1 rounded font-medium active:scale-95 cursor-pointer">
        Durdur
      </button>
    </div>
  );
}

function ConfigView() {
  const [outputPath, setOutputPath] = useState(() => localStorage.getItem("cfg_output_path") || "D:\\Videolar");
  const [preset, setPreset] = useState(() => localStorage.getItem("cfg_preset") || "ultrafast");
  const [encoder, setEncoder] = useState(() => localStorage.getItem("cfg_encoder") || "cpu");
  const [exportGif, setExportGif] = useState(() => localStorage.getItem("cfg_gif") === "true");
  const [recordSystemAudio, setRecordSystemAudio] = useState(() => localStorage.getItem("cfg_sys_audio") === "true");
  const [recordMic, setRecordMic] = useState(() => localStorage.getItem("cfg_mic_audio") === "true");
  const [sysAudioDevice, setSysAudioDevice] = useState(() => localStorage.getItem("cfg_sys_dev") || "");
  const [micDevice, setMicDevice] = useState(() => localStorage.getItem("cfg_mic_dev") || "");
  
  const [sysDevices, setSysDevices] = useState<string[]>([]);
  const [micDevices, setMicDevices] = useState<string[]>([]);

  const [shortcut, setShortcut] = useState(() => localStorage.getItem("cfg_shortcut") || "CommandOrControl+Shift+R");
  const [countdown, setCountdown] = useState(() => parseInt(localStorage.getItem("cfg_countdown") || "0"));
  const [drawMouse, setDrawMouse] = useState(() => localStorage.getItem("cfg_draw_mouse") !== "false");
  const [filenameTemplate, setFilenameTemplate] = useState(() => localStorage.getItem("cfg_filename_tpl") || "COSRecorder_%datetime%");
  const [diskFree, setDiskFree] = useState<number | null>(null);
  const [recording, setRecording] = useState(false);
  const [caps, setCaps] = useState<EncoderCaps | null>(null);
  const [showWelcome, setShowWelcome] = useState(() => localStorage.getItem("cfg_welcomed") !== "true");

  const refreshDevices = async () => {
    try {
      const outputs = await invoke<string[]>("get_output_devices");
      const inputs = await invoke<string[]>("get_input_devices");

      setSysDevices(outputs);
      setMicDevices(inputs);

      if (outputs.length > 0 && !outputs.includes(sysAudioDevice)) {
        setSysAudioDevice(outputs[0]);
      }
      if (inputs.length > 0 && !inputs.includes(micDevice)) {
        setMicDevice(inputs[0]);
      }
    } catch (e) {
      console.error("Cihazlar alınamadı:", e);
    }
  };

  useEffect(() => {
    refreshDevices();
    invoke<EncoderCaps>("probe_encoders").then(setCaps).catch(() => setCaps({ cpu: true, nvenc: false, amf: false, draw_mouse_supported: true }));
  }, []);

  useEffect(() => {
    invoke<number>("check_disk_space", { dir: outputPath }).then(setDiskFree).catch(() => setDiskFree(null));
  }, [outputPath]);

  useEffect(() => {
    if (!recording) return;
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault(); e.stopPropagation();
      if (e.key === "Escape") { setRecording(false); return; }
      if (e.key === "Backspace" || e.key === "Delete") { setShortcut(""); setRecording(false); return; }
      if (["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(e.key)) return;
      const parts: string[] = [];
      if (e.ctrlKey || e.metaKey) parts.push("CommandOrControl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      let keyName = e.key;
      if (keyName === " ") keyName = "Space";
      else if (keyName.length === 1) keyName = keyName.toUpperCase();
      else if (keyName.startsWith("Arrow")) keyName = keyName.replace("Arrow", "");
      parts.push(keyName);
      if (parts.length < 2 && !/^F\d+$/.test(keyName)) return;
      setShortcut(parts.join("+"));
      setRecording(false);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [recording]);

  const saveSettings = async () => {
    localStorage.setItem("cfg_output_path", outputPath);
    localStorage.setItem("cfg_preset", preset);
    localStorage.setItem("cfg_encoder", encoder);
    localStorage.setItem("cfg_gif", String(exportGif));
    localStorage.setItem("cfg_sys_audio", String(recordSystemAudio));
    localStorage.setItem("cfg_mic_audio", String(recordMic));
    localStorage.setItem("cfg_sys_dev", sysAudioDevice);
    localStorage.setItem("cfg_mic_dev", micDevice);
    localStorage.setItem("cfg_shortcut", shortcut);
    localStorage.setItem("cfg_countdown", String(countdown));
    localStorage.setItem("cfg_draw_mouse", String(drawMouse));
    localStorage.setItem("cfg_filename_tpl", filenameTemplate);
    localStorage.setItem("cfg_welcomed", "true");
    if (shortcut) {
      try {
        await invoke("disable_shortcut");
        await invoke("set_shortcut", { accelerator: shortcut });
      } catch (e) {
        alert(`Kısayol kaydedilemedi (${shortcut}). Başka bir uygulama tarafından kullanılıyor olabilir.`);
        const old = localStorage.getItem("cfg_shortcut") || "";
        if (old) invoke("set_shortcut", { accelerator: old }).catch(() => {});
        return;
      }
    }
    await getCurrentWindow().hide();
  };

  const copyLog = async () => {
    try {
      const log = await invoke<string>("read_recent_log", { lines: 500 });
      await navigator.clipboard.writeText(log);
      alert("Son 500 satır log panoya kopyalandı.");
    } catch (e) { alert("Log okunamadı: " + e); }
  };

  const diskGB = diskFree !== null ? (diskFree / 1024 / 1024 / 1024).toFixed(1) : "?";

  if (showWelcome) {
    return (
      <div className="w-screen h-screen bg-zinc-950 text-zinc-100 p-8 flex flex-col justify-between font-sans select-none">
        <div>
          <h1 className="text-2xl font-bold mb-2">COSRecorder'a Hoş Geldiniz</h1>
          <p className="text-zinc-400 text-sm mb-6">Birkaç saniyelik kurulum.</p>
          <div className="flex flex-col gap-4">
            <div className="bg-zinc-900 border border-zinc-800 rounded-lg p-4">
              <div className="text-xs text-zinc-500 mb-1">KAYIT KLASÖRÜ</div>
              <input type="text" value={outputPath} onChange={(e) => setOutputPath(e.target.value)}
                className="w-full bg-zinc-950 border border-zinc-700 rounded px-3 py-2 font-mono text-sm focus:outline-none focus:border-blue-500" />
            </div>
            {caps && (
              <div className="bg-zinc-900 border border-zinc-800 rounded-lg p-4">
                <div className="text-xs text-zinc-500 mb-2">DONANIM</div>
                <div className="flex flex-wrap gap-2 text-xs">
                  <span className={`px-2 py-0.5 rounded ${caps.cpu ? "bg-green-900/60 text-green-300" : "bg-red-900/60 text-red-300"}`}>CPU x264</span>
                  <span className={`px-2 py-0.5 rounded ${caps.nvenc ? "bg-green-900/60 text-green-300" : "bg-zinc-800 text-zinc-500"}`}>NVIDIA NVENC</span>
                  <span className={`px-2 py-0.5 rounded ${caps.amf ? "bg-green-900/60 text-green-300" : "bg-zinc-800 text-zinc-500"}`}>AMD AMF</span>
                </div>
              </div>
            )}
          </div>
        </div>
        <button onClick={() => setShowWelcome(false)}
          className="bg-blue-600 hover:bg-blue-500 text-white font-medium text-sm px-5 py-2.5 rounded-lg self-end active:scale-95 cursor-pointer">
          Başla
        </button>
      </div>
    );
  }

  return (
    <div className="w-screen h-screen bg-zinc-950 text-zinc-100 p-6 flex flex-col gap-5 font-sans select-none overflow-y-auto">
      <div className="flex items-center justify-between pb-4 border-b border-zinc-800">
        <h1 className="text-xl font-bold flex items-center gap-2">
          <span className="w-2.5 h-2.5 rounded-full bg-blue-500" /> COSRecorder Ayarlar
        </h1>
        <span className="text-xs text-zinc-500 font-mono">v2.0.0</span>
      </div>

      <div className="flex flex-col gap-1.5">
        <label className="text-zinc-400 text-xs font-semibold tracking-wide">KAYIT KLASÖRÜ</label>
        <input type="text" value={outputPath} onChange={(e) => setOutputPath(e.target.value)}
          className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 font-mono text-sm focus:outline-none focus:border-blue-500" />
        <span className="text-xs text-zinc-500">Boş alan: {diskGB} GB</span>
      </div>

      <div className="flex flex-col gap-1.5">
        <label className="text-zinc-400 text-xs font-semibold tracking-wide">DOSYA ADI ŞABLONU</label>
        <input type="text" value={filenameTemplate} onChange={(e) => setFilenameTemplate(e.target.value)}
          className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 font-mono text-sm focus:outline-none focus:border-blue-500" />
        <span className="text-xs text-zinc-500">Kullanılabilir: %datetime%, %date%, %time%</span>
      </div>

      <div className="flex flex-col gap-3 p-4 bg-zinc-900/60 border border-zinc-800 rounded-xl">
        <div className="flex justify-between items-center">
          <span className="text-zinc-200 font-semibold text-sm">🔊 Ses Ayarları</span>
          <button onClick={refreshDevices}
            className="text-xs text-zinc-400 hover:text-zinc-200 underline cursor-pointer">Yenile</button>
        </div>

        <div className="flex flex-col gap-1.5">
          <div className="flex justify-between items-center">
            <label className="text-zinc-400 text-xs">Sistem Sesi (Çıkış)</label>
            <label className="flex items-center gap-1.5 text-xs text-zinc-400 cursor-pointer">
              <input type="checkbox" checked={recordSystemAudio} onChange={(e) => setRecordSystemAudio(e.target.checked)}
                className="rounded accent-blue-500 w-3.5 h-3.5" /> Etkin
            </label>
          </div>
          <select disabled={!recordSystemAudio} value={sysAudioDevice} onChange={(e) => setSysAudioDevice(e.target.value)}
            className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 text-sm disabled:opacity-40">
            {sysDevices.length === 0 && <option value="">(cihaz bulunamadı)</option>}
            {sysDevices.map((n) => <option key={n} value={n}>{n}</option>)}
          </select>
        </div>

        <div className="flex flex-col gap-1.5">
          <div className="flex justify-between items-center">
            <label className="text-zinc-400 text-xs">Mikrofon (Giriş)</label>
            <label className="flex items-center gap-1.5 text-xs text-zinc-400 cursor-pointer">
              <input type="checkbox" checked={recordMic} onChange={(e) => setRecordMic(e.target.checked)}
                className="rounded accent-blue-500 w-3.5 h-3.5" /> Etkin
            </label>
          </div>
          <select disabled={!recordMic} value={micDevice} onChange={(e) => setMicDevice(e.target.value)}
            className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 text-sm disabled:opacity-40">
            {micDevices.length === 0 && <option value="">(cihaz bulunamadı)</option>}
            {micDevices.map((n) => <option key={n} value={n}>{n}</option>)}
          </select>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div className="flex flex-col gap-1.5">
          <label className="text-zinc-400 text-xs">KODLAYICI</label>
          <select value={encoder} onChange={(e) => setEncoder(e.target.value)}
            className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 text-sm">
            <option value="cpu">CPU (x264)</option>
            {(!caps || caps.nvenc) && <option value="nvenc">NVIDIA NVENC</option>}
            {(!caps || caps.amf) && <option value="amf">AMD AMF</option>}
          </select>
        </div>
        <div className="flex flex-col gap-1.5">
          <label className="text-zinc-400 text-xs">FPS KİLİDİ</label>
          <div className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 text-sm flex items-center justify-between text-zinc-300 font-mono">
            <span>60 FPS</span>
            <span className="text-[10px] bg-blue-900/60 text-blue-300 px-1.5 py-0.5 rounded">Sabit Kalite</span>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div className="flex flex-col gap-1.5">
          <label className="text-zinc-400 text-xs">PRESET</label>
          <select value={preset} onChange={(e) => setPreset(e.target.value)}
            className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 text-sm">
            <option value="ultrafast">Ultrafast</option><option value="superfast">Superfast</option><option value="veryfast">Veryfast</option>
          </select>
        </div>
        <div className="flex flex-col gap-1.5">
          <label className="text-zinc-400 text-xs">GERİ SAYIM</label>
          <select value={countdown} onChange={(e) => setCountdown(parseInt(e.target.value))}
            className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-2 text-sm">
            <option value="0">Kapalı</option><option value="3">3 saniye</option><option value="5">5 saniye</option><option value="10">10 saniye</option>
          </select>
        </div>
      </div>

      <div className="flex flex-col gap-1.5">
        <label className="text-zinc-400 text-xs">GLOBAL KISAYOL</label>
        <button type="button" onClick={() => setRecording((r) => !r)} onBlur={() => setRecording(false)}
          className={`w-full text-left rounded-lg px-3 py-2 font-mono text-sm border transition-colors cursor-pointer ${
            recording ? "bg-blue-600/20 border-blue-500 text-blue-200 animate-pulse" : "bg-zinc-900 border-zinc-800 hover:border-zinc-700"
          }`}>
          {recording ? "Tuş kombinasyonuna basın… (Esc = iptal, Backspace = temizle)"
            : shortcut
              ? <span className="inline-flex items-center gap-1">
                  {shortcut.split("+").map((p, i) => (
                    <span key={i} className="inline-flex items-center">
                      {i > 0 && <span className="text-zinc-500 mx-0.5">+</span>}
                      <kbd className="px-2 py-0.5 bg-zinc-800 border border-zinc-700 rounded text-xs">
                        {p === "CommandOrControl" ? (navigator.platform.includes("Mac") ? "⌘" : "Ctrl") : p}
                      </kbd>
                    </span>
                  ))}
                </span>
              : <span className="text-zinc-500">Atanmadı</span>}
        </button>
      </div>

      <div className="flex flex-col gap-2 p-4 bg-zinc-900/60 border border-zinc-800 rounded-xl">
        <label className="flex items-center gap-2 text-sm text-zinc-300 cursor-pointer">
          <input type="checkbox" checked={exportGif} onChange={(e) => setExportGif(e.target.checked)}
            className="rounded accent-blue-500 w-3.5 h-3.5" /> MP4 yerine GIF olarak kaydet
        </label>
        <label className="flex items-center gap-2 text-sm text-zinc-300 cursor-pointer">
          <input type="checkbox" checked={drawMouse} onChange={(e) => setDrawMouse(e.target.checked)}
            className="rounded accent-blue-500 w-3.5 h-3.5" /> Fare imlecini kayıtta göster
        </label>
      </div>

      <div className="flex items-center justify-between pt-3 border-t border-zinc-800 mt-auto">
        <button onClick={copyLog}
          className="text-xs text-zinc-500 hover:text-zinc-300 underline cursor-pointer">
          Hata ayıklama logunu kopyala
        </button>
        <button onClick={saveSettings}
          className="bg-blue-600 hover:bg-blue-500 text-white text-sm font-medium px-5 py-2 rounded-lg active:scale-95 cursor-pointer">
          Kaydet ve Kapat
        </button>
      </div>
    </div>
  );
}

// ==================== GALLERY ====================
function GalleryView() {
  const [files, setFiles] = useState<RecordingFile[]>([]);
  const [orphans, setOrphans] = useState<OrphanRecord[]>([]);
  const dir = localStorage.getItem("cfg_output_path") || "D:\\Videolar";

  const refresh = async () => {
    try {
      const [list, orphanList] = await Promise.all([
        invoke<RecordingFile[]>("list_recordings", { dir }),
        invoke<OrphanRecord[]>("recover_temp_files", { dir }),
      ]);
      setFiles(list); setOrphans(orphanList);
    } catch (e) { console.error(e); }
  };

  useEffect(() => { refresh(); }, []);

  const fmtSize = (b: number) => b > 1024 * 1024 ? (b / 1024 / 1024).toFixed(1) + " MB" : (b / 1024).toFixed(0) + " KB";
  const fmtDate = (ts: number) => new Date(ts * 1000).toLocaleString("tr-TR");

  return (
    <div className="w-screen h-screen bg-zinc-950 text-zinc-100 p-5 flex flex-col gap-3 font-sans overflow-hidden">
      <div className="flex items-center justify-between pb-3 border-b border-zinc-800">
        <h1 className="text-base font-bold flex items-center gap-2">
          <span className="w-2 h-2 rounded-full bg-purple-500" /> Galeri
        </h1>
        <div className="flex gap-2">
          <button onClick={refresh} className="bg-zinc-800 hover:bg-zinc-700 text-xs px-3 py-1.5 rounded-lg cursor-pointer">Yenile</button>
          <button onClick={() => invoke("open_in_folder", { path: dir })}
            className="bg-zinc-800 hover:bg-zinc-700 text-xs px-3 py-1.5 rounded-lg cursor-pointer">Klasörü Aç</button>
        </div>
      </div>

      {orphans.length > 0 && (
        <div className="bg-amber-950/50 border border-amber-700/70 rounded-lg p-3 flex flex-col gap-2">
          <div className="text-amber-200 text-xs font-semibold">⚠️ {orphans.length} yarım kayıt bulundu</div>
          {orphans.map((o) => (
            <div key={o.temp_video} className="flex items-center gap-2 text-[11px] text-amber-100">
              <span className="font-mono truncate flex-1">{o.temp_video.split(/[\\/]/).pop()}</span>
              <button onClick={async () => { await invoke("recover_recording", { tempVideo: o.temp_video, tempSys: o.temp_sys, tempMic: o.temp_mic, finalPath: o.suggested_final }); refresh(); }}
                className="bg-green-700 hover:bg-green-600 text-white px-2 py-0.5 rounded cursor-pointer">Kurtar</button>
              <button onClick={async () => { if (confirm("Silinsin mi?")) { await invoke("discard_orphan", { tempVideo: o.temp_video, tempSys: o.temp_sys, tempMic: o.temp_mic }); refresh(); } }}
                className="bg-zinc-800 hover:bg-zinc-700 px-2 py-0.5 rounded cursor-pointer">Sil</button>
            </div>
          ))}
        </div>
      )}

      <div className="flex-1 overflow-y-auto grid grid-cols-2 gap-2">
        {files.length === 0 && <div className="col-span-2 text-center text-zinc-500 text-sm mt-10">Henüz kayıt yok.</div>}
        {files.map((f) => (
          <div key={f.path} className="bg-zinc-900 border border-zinc-800 rounded-lg p-3 flex flex-col gap-1.5">
            <div className="font-mono text-[11px] truncate" title={f.name}>{f.name}</div>
            <div className="text-[10px] text-zinc-500">{fmtSize(f.size)} · {fmtDate(f.modified)}</div>
            <div className="flex gap-1.5 mt-1">
              <button onClick={() => invoke("open_file", { path: f.path })} className="flex-1 bg-blue-600 hover:bg-blue-500 text-[10px] px-2 py-1 rounded cursor-pointer">Oynat</button>
              <button onClick={() => invoke("open_in_folder", { path: f.path })} className="bg-zinc-800 hover:bg-zinc-700 text-[10px] px-2 py-1 rounded cursor-pointer">📁</button>
              <button onClick={async () => { if (confirm(`Silinsin mi?\n${f.name}`)) { await invoke("delete_recording", { path: f.path }); refresh(); } }}
                className="bg-red-900/60 hover:bg-red-800 text-[10px] px-2 py-1 rounded cursor-pointer">Sil</button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ==================== OVERLAY ====================
type Region = { x: number; y: number; w: number; h: number };

function OverlayRecorderView() {
  const [isSelecting, setIsSelecting] = useState(false);
  const [startPos, setStartPos] = useState({ x: 0, y: 0 });
  const [currentPos, setCurrentPos] = useState({ x: 0, y: 0 });
  const [selection, setSelection] = useState<Region | null>(null);
  const [lastRightClickTime, setLastRightClickTime] = useState(0);
  const [countdownLeft, setCountdownLeft] = useState(0);
  const [lastRegion, setLastRegion] = useState<Region | null>(null);
  const [monitor, setMonitor] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    const readLast = () => {
      const saved = localStorage.getItem("last_region");
      if (saved) { try { setLastRegion(JSON.parse(saved)); } catch {} } else setLastRegion(null);
    };
    readLast();
    const sc = localStorage.getItem("cfg_shortcut") || "CommandOrControl+Shift+R";
    invoke("set_shortcut", { accelerator: sc }).catch(console.warn);
    currentMonitor().then((m) => m && setMonitor({ w: m.size.width, h: m.size.height })).catch(() => {});
    const un = listen("recording-stopped", readLast);
    return () => { un.then((f) => f()); };
  }, []);

  useEffect(() => {
    let focused = true;
    const onFocus = async () => {
      if (!focused) {
        focused = true;
        const sc = localStorage.getItem("cfg_shortcut") || "CommandOrControl+Shift+R";
        try { await invoke("set_shortcut", { accelerator: sc }); } catch {}
      }
    };
    const onBlur = async () => {
      if (focused) { focused = false; try { await invoke("disable_shortcut"); } catch {} }
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    return () => { window.removeEventListener("focus", onFocus); window.removeEventListener("blur", onBlur); };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { setSelection(null); setIsSelecting(false); }
      else if (e.key === "Enter" && selection) startRecording();
      else if (e.key === " ") {
        e.preventDefault();
        if (monitor) setSelection({ x: 0, y: 0, w: monitor.w, h: monitor.h });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection, monitor]);

  useEffect(() => {
    const preventCtx = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", preventCtx);
    return () => document.removeEventListener("contextmenu", preventCtx);
  }, []);

  const handleMouseDown = (e: React.MouseEvent) => {
    if (e.button === 2) {
      const now = Date.now();
      if (now - lastRightClickTime < 300) {
        if (monitor) {
          const w = monitor.w % 2 === 0 ? monitor.w : monitor.w - 1;
          const h = monitor.h % 2 === 0 ? monitor.h : monitor.h - 1;
          setSelection({ x: 0, y: 0, w, h });
        }
        setLastRightClickTime(0); return;
      }
      setLastRightClickTime(now); return;
    }
    if (e.button === 0) {
      setIsSelecting(true);
      setStartPos({ x: e.clientX, y: e.clientY });
      setCurrentPos({ x: e.clientX, y: e.clientY });
      setSelection(null);
    }
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (isSelecting) setCurrentPos({ x: e.clientX, y: e.clientY });
  };

  const handleMouseUp = (e: React.MouseEvent) => {
    if (e.button !== 0 || !isSelecting) return;
    setIsSelecting(false);
    const x = Math.min(startPos.x, currentPos.x);
    const y = Math.min(startPos.y, currentPos.y);
    const w = Math.abs(currentPos.x - startPos.x);
    const h = Math.abs(currentPos.y - startPos.y);
    if (w > 10 && h > 10) {
      setSelection({ x: Math.round(x), y: Math.round(y), w: w % 2 === 0 ? w : w + 1, h: h % 2 === 0 ? h : h + 1 });
    }
  };

  const startRecording = async () => {
    if (!selection) return;
    const cd = parseInt(localStorage.getItem("cfg_countdown") || "0");
    if (cd > 0) {
      for (let i = cd; i > 0; i--) { setCountdownLeft(i); await new Promise((r) => setTimeout(r, 1000)); }
      setCountdownLeft(0);
    }

    localStorage.setItem("last_region", JSON.stringify(selection));

    try {
      await invoke("start_capture", {
        x: selection.x, y: selection.y, w: selection.w, h: selection.h,
        preset: localStorage.getItem("cfg_preset") || "ultrafast",
        encoder: localStorage.getItem("cfg_encoder") || "cpu",
        exportGif: localStorage.getItem("cfg_gif") === "true",
        outputDir: localStorage.getItem("cfg_output_path") || "D:\\Videolar",
        recordSystemAudio: localStorage.getItem("cfg_sys_audio") === "true",
        recordMic: localStorage.getItem("cfg_mic_audio") === "true",
        sysAudioDevice: localStorage.getItem("cfg_sys_dev") || "",
        micDevice: localStorage.getItem("cfg_mic_dev") || "",
        drawMouse: localStorage.getItem("cfg_draw_mouse") !== "false",
        filenameTemplate: localStorage.getItem("cfg_filename_tpl") || "COSRecorder_%datetime%",
      });
      setSelection(null);
    } catch (error) {
      alert("Kayıt Hatası: " + (typeof error === "object" ? JSON.stringify(error) : String(error)));
    }
  };

  const applyPreset = (name: "full" | "1080p" | "720p" | "last") => {
    if (name === "last" && lastRegion) { setSelection(lastRegion); return; }
    if (!monitor) return;
    if (name === "full") {
      setSelection({ x: 0, y: 0, w: monitor.w % 2 === 0 ? monitor.w : monitor.w - 1, h: monitor.h % 2 === 0 ? monitor.h : monitor.h - 1 });
    } else {
      const [w, h] = name === "1080p" ? [1920, 1080] : [1280, 720];
      const x = Math.max(0, Math.round((monitor.w - w) / 2));
      const y = Math.max(0, Math.round((monitor.h - h) / 2));
      setSelection({ x, y, w, h });
    }
  };

  const boxX = Math.min(startPos.x, currentPos.x);
  const boxY = Math.min(startPos.y, currentPos.y);
  const boxW = Math.abs(currentPos.x - startPos.x);
  const boxH = Math.abs(currentPos.y - startPos.y);

  return (
    <div className="w-screen h-screen bg-black/45 cursor-crosshair select-none relative overflow-hidden"
      onMouseDown={handleMouseDown} onMouseMove={handleMouseMove} onMouseUp={handleMouseUp}>

      {countdownLeft > 0 && (
        <div className="absolute inset-0 flex items-center justify-center bg-black/70 z-50 pointer-events-none">
          <span className="text-white text-9xl font-bold animate-pulse">{countdownLeft}</span>
        </div>
      )}

      {!isSelecting && !selection && (
        <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 bg-zinc-900/95 backdrop-blur-md text-white px-6 py-5 rounded-xl shadow-2xl border border-zinc-700/50 pointer-events-auto text-center max-w-md">
          <div className="font-semibold text-lg">COSRecorder Hazır</div>
          <div className="text-zinc-400 text-xs mt-1 mb-3">Sürükle · Space = monitör · Çift sağ tık = tam ekran</div>
          <div className="flex flex-wrap gap-1.5 justify-center">
            <button onClick={() => applyPreset("full")} className="bg-zinc-800 hover:bg-zinc-700 text-white text-xs px-3 py-1.5 rounded cursor-pointer">Tam ekran</button>
            <button onClick={() => applyPreset("1080p")} className="bg-zinc-800 hover:bg-zinc-700 text-white text-xs px-3 py-1.5 rounded cursor-pointer">1920×1080</button>
            <button onClick={() => applyPreset("720p")} className="bg-zinc-800 hover:bg-zinc-700 text-white text-xs px-3 py-1.5 rounded cursor-pointer">1280×720</button>
            {lastRegion && (
              <button onClick={() => applyPreset("last")} className="bg-blue-600 hover:bg-blue-500 text-white text-xs px-3 py-1.5 rounded cursor-pointer">
                Son alan ({lastRegion.w}×{lastRegion.h})
              </button>
            )}
          </div>
        </div>
      )}

      {isSelecting && (
        <div className="absolute border-2 border-blue-500 bg-blue-500/20" style={{ left: boxX, top: boxY, width: boxW, height: boxH }}>
          <span className="absolute -bottom-7 right-0 text-white bg-blue-600 px-2 py-0.5 rounded text-xs font-mono">{boxW} × {boxH}</span>
        </div>
      )}

      {selection && !isSelecting && countdownLeft === 0 && (
        <div className="absolute border-2 border-green-500 bg-green-500/10" style={{ left: selection.x, top: selection.y, width: selection.w, height: selection.h }}>
          <div onMouseDown={(e) => e.stopPropagation()}
            className="absolute top-2 left-2 flex items-center gap-2 bg-zinc-900/95 border border-zinc-700/80 p-1.5 rounded-lg shadow-2xl backdrop-blur-md z-50">
            <button onClick={startRecording}
              className="bg-red-600 hover:bg-red-500 text-white px-3 py-1 rounded text-xs font-medium flex items-center gap-1.5 cursor-pointer">
              <div className="w-2 h-2 rounded-full bg-white animate-pulse" /> Kaydı Başlat
            </button>
            <button onClick={() => setSelection(null)}
              className="bg-zinc-800 hover:bg-zinc-700 text-zinc-300 px-2.5 py-1 rounded text-xs cursor-pointer">İptal</button>
            <span className="text-zinc-400 text-xs px-1 font-mono">{selection.w}×{selection.h}</span>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

function App() {
  const [route, setRoute] = useState(window.location.hash);

  useEffect(() => {
    const handleHashChange = () => setRoute(window.location.hash);
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  if (route === "#/config") {
    return <ConfigView />;
  }

  return <OverlayRecorderView />;
}

// === AYARLAR PENCERESİ ===
function ConfigView() {
  const [outputPath, setOutputPath] = useState(() => localStorage.getItem("cfg_output_path") || "D:\\Videolar");
  const [fps, setFps] = useState(() => localStorage.getItem("cfg_fps") || "30");
  const [preset, setPreset] = useState(() => localStorage.getItem("cfg_preset") || "ultrafast");
  const [encoder, setEncoder] = useState(() => localStorage.getItem("cfg_encoder") || "cpu");
  const [exportGif, setExportGif] = useState(() => localStorage.getItem("cfg_gif") === "true");

  const saveSettings = () => {
    localStorage.setItem("cfg_output_path", outputPath); // <-- Klasör yolunu kaydet
    localStorage.setItem("cfg_fps", fps);
    localStorage.setItem("cfg_preset", preset);
    localStorage.setItem("cfg_encoder", encoder);
    localStorage.setItem("cfg_gif", String(exportGif));
    getCurrentWindow().close();
  };

  return (
    <div className="w-screen h-screen bg-zinc-950 text-zinc-100 p-6 flex flex-col justify-between font-sans select-none">
      <div>
        <div className="flex items-center justify-between pb-4 border-b border-zinc-800">
          <h1 className="text-lg font-bold tracking-wide text-zinc-100 flex items-center gap-2">
            <span className="w-2.5 h-2.5 rounded-full bg-blue-500"></span>
            COSRecorder Gelişmiş Ayarlar
          </h1>
          <span className="text-xs text-zinc-500 font-mono">v0.2.0</span>
        </div>

        <div className="mt-5 space-y-4 text-sm">
          {/* Kayıt Klasörü */}
          <div className="flex flex-col gap-1.5">
            <label className="text-zinc-400 font-medium text-xs">KAYIT KLASÖRÜ</label>
            <input 
              type="text" 
              value={outputPath} 
              onChange={(e) => setOutputPath(e.target.value)}
              className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-1.5 font-mono text-xs text-zinc-200 focus:outline-none focus:border-blue-500"
            />
          </div>

          {/* Donanım Hızlandırma & FPS */}
          <div className="grid grid-cols-2 gap-4">
            <div className="flex flex-col gap-1.5">
              <label className="text-zinc-400 font-medium text-xs">KODLAYICI (ENCODER)</label>
              <select 
                value={encoder} 
                onChange={(e) => setEncoder(e.target.value)}
                className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-blue-500 cursor-pointer"
              >
                <option value="cpu">CPU (x264 - Standart)</option>
                <option value="nvenc">NVIDIA NVENC (Donanım)</option>
                <option value="amf">AMD AMF (Donanım)</option>
              </select>
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-zinc-400 font-medium text-xs">KARE HIZI (FPS)</label>
              <select 
                value={fps} 
                onChange={(e) => setFps(e.target.value)}
                className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-blue-500 cursor-pointer"
              >
                <option value="30">30 FPS</option>
                <option value="60">60 FPS (Akıcı)</option>
                <option value="120">120 FPS (Oyun)</option>
              </select>
            </div>
          </div>

          {/* Preset & GIF Mode */}
          <div className="grid grid-cols-2 gap-4">
            <div className="flex flex-col gap-1.5">
              <label className="text-zinc-400 font-medium text-xs">ENCODER PRESET</label>
              <select 
                value={preset} 
                onChange={(e) => setPreset(e.target.value)}
                className="bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-blue-500 cursor-pointer"
              >
                <option value="ultrafast">Ultrafast (Düşük CPU)</option>
                <option value="superfast">Superfast</option>
                <option value="veryfast">Veryfast (Yüksek Kalite)</option>
              </select>
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-zinc-400 font-medium text-xs">ÇIKTI FORMATI</label>
              <label className="flex items-center gap-2 bg-zinc-900 border border-zinc-800 rounded-lg px-3 py-1.5 text-xs text-zinc-300 cursor-pointer">
                <input 
                  type="checkbox" 
                  checked={exportGif} 
                  onChange={(e) => setExportGif(e.target.checked)}
                  className="rounded accent-blue-500"
                />
                MP4 yerine GIF Olarak Kaydet
              </label>
            </div>
          </div>
        </div>
      </div>

      <div className="flex justify-end gap-2 pt-4 border-t border-zinc-800">
        <button 
          onClick={saveSettings}
          className="bg-blue-600 hover:bg-blue-500 text-white font-medium text-xs px-4 py-2 rounded-lg transition-colors cursor-pointer"
        >
          Kaydet ve Kapat
        </button>
      </div>
    </div>
  );
}

// === OVERLAY KAYIT BİLEŞENİ ===
function OverlayRecorderView() {
  const [isSelecting, setIsSelecting] = useState(false);
  const [startPos, setStartPos] = useState({ x: 0, y: 0 });
  const [currentPos, setCurrentPos] = useState({ x: 0, y: 0 });
  const [selection, setSelection] = useState<{ x: number; y: number; w: number; h: number } | null>(null);
  const [isRecording, setIsRecording] = useState(false);

  useEffect(() => {
    const handleContextMenu = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", handleContextMenu);

    const unlisten = listen("recording-stopped", async () => {
      setIsRecording(false);
      setSelection(null);
      const appWindow = getCurrentWindow();
      await appWindow.setIgnoreCursorEvents(false);
    });

    return () => {
      document.removeEventListener("contextmenu", handleContextMenu);
      unlisten.then((f) => f());
    };
  }, []);

  const handleMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0 || isRecording) return;
    setIsSelecting(true);
    setStartPos({ x: e.clientX, y: e.clientY });
    setCurrentPos({ x: e.clientX, y: e.clientY });
    setSelection(null);
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (!isSelecting) return;
    setCurrentPos({ x: e.clientX, y: e.clientY });
  };

  const handleMouseUp = () => {
    if (!isSelecting) return;
    setIsSelecting(false);
    
    const x = Math.min(startPos.x, currentPos.x);
    const y = Math.min(startPos.y, currentPos.y);
    const w = Math.abs(currentPos.x - startPos.x);
    const h = Math.abs(currentPos.y - startPos.y);
    
    if (w > 10 && h > 10) {
      setSelection({ 
        x: Math.round(x), 
        y: Math.round(y), 
        w: w % 2 === 0 ? w : w + 1, 
        h: h % 2 === 0 ? h : h + 1 
      });
    }
  };

  const startRecording = async () => {
  if (!selection) return;
  setIsRecording(true);

  const outputDir = localStorage.getItem("cfg_output_path") || "D:\\Videolar";
  const fps = localStorage.getItem("cfg_fps") || "30";
  const preset = localStorage.getItem("cfg_preset") || "ultrafast";
  const encoder = localStorage.getItem("cfg_encoder") || "cpu";
  const exportGif = localStorage.getItem("cfg_gif") === "true";

  try {
    await invoke("start_capture", {
      x: Math.round(selection.x),
      y: Math.round(selection.y),
      w: Math.round(selection.w),
      h: Math.round(selection.h),
      fps,
      preset,
      encoder,
      exportGif,
      outputDir,
    });

    const appWindow = getCurrentWindow();
    await appWindow.setIgnoreCursorEvents(true);
    } catch (error) {
      console.error("Kayıt başlatılamadı:", error);
      alert("Kayıt Hatası: " + (typeof error === "object" ? JSON.stringify(error) : String(error)));
      setIsRecording(false);
    }
};

  const cancelSelection = () => {
    setSelection(null);
    setIsRecording(false);
  };

  const boxX = Math.min(startPos.x, currentPos.x);
  const boxY = Math.min(startPos.y, currentPos.y);
  const boxW = Math.abs(currentPos.x - startPos.x);
  const boxH = Math.abs(currentPos.y - startPos.y);

  return (
    <div 
      className={`w-screen h-screen ${isRecording ? "bg-transparent" : "bg-black/45"} cursor-crosshair select-none relative`}
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
    >
      {!isSelecting && !selection && !isRecording && (
        <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 bg-zinc-900/95 backdrop-blur-md text-white px-6 py-4 rounded-xl shadow-2xl border border-zinc-700/50 flex flex-col items-center pointer-events-none">
          <span className="font-semibold tracking-wide text-lg">COSRecorder Hazır</span>
          <span className="text-zinc-400 text-sm mt-1">Kayıt alanını seçmek için sürükleyin</span>
        </div>
      )}

      {isSelecting && (
        <div 
          className="absolute border-2 border-blue-500 bg-blue-500/20 shadow-[0_0_15px_rgba(59,130,246,0.5)]"
          style={{ left: boxX, top: boxY, width: boxW, height: boxH }}
        >
          <span className="absolute -bottom-7 right-0 text-white bg-blue-600 px-2 py-0.5 rounded text-xs font-mono shadow-md">
            {boxW} x {boxH}
          </span>
        </div>
      )}

      {selection && !isSelecting && (
        <div 
          className={`absolute border-2 ${isRecording ? "border-red-500 animate-pulse" : "border-green-500 bg-green-500/10"} shadow-lg`}
          style={{ left: selection.x, top: selection.y, width: selection.w, height: selection.h }}
        >
          {!isRecording ? (
            <div 
              onMouseDown={(e) => e.stopPropagation()} 
              className="absolute top-2 left-2 flex items-center gap-2 bg-zinc-900/95 border border-zinc-700/80 p-1.5 rounded-lg shadow-2xl backdrop-blur-md z-50"
            >
              <button 
                onClick={startRecording}
                className="bg-red-600 hover:bg-red-500 text-white px-3 py-1 rounded text-xs font-medium transition-colors flex items-center gap-1.5 cursor-pointer"
              >
                <div className="w-2 h-2 rounded-full bg-white animate-pulse" />
                Kaydı Başlat
              </button>
              <button 
                onClick={cancelSelection}
                className="bg-zinc-800 hover:bg-zinc-700 text-zinc-300 px-2.5 py-1 rounded text-xs transition-colors cursor-pointer"
              >
                İptal
              </button>
              <span className="text-zinc-400 text-xs px-1 font-mono">
                {selection.w}x{selection.h}
              </span>
            </div>
          ) : (
            <div className="absolute top-2 left-2 bg-red-600 text-white px-3 py-1 rounded-md text-xs font-semibold shadow-lg flex items-center gap-2 z-50">
              <div className="w-2 h-2 rounded-full bg-white animate-ping" />
              KAYDEDİLİYOR (Durdurmak için Ctrl+Shift+R)
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default App;
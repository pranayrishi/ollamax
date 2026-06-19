// @ts-check
"use strict";

// Voice-activated demo navigation (Phase 2). Push-to-talk: a small webview panel
// captures one short utterance via getUserMedia + Web Audio (16 kHz mono WAV,
// fully on-device — NO Web Speech API, nothing leaves the machine), sends the
// WAV to this host, which transcribes it with a LOCAL whisper.cpp CLI, asks the
// engine to resolve the transcript to a code location (/api/voice/locate over the
// graph), and reveals that file:line in the editor. Heard -> target is shown with
// Undo / Next. A status-bar item reflects Idle/Listening/Transcribing/Resolving.
//
// HONEST: STT requires a local whisper.cpp binary + a ggml model. If absent, the
// flow degrades to a clear, actionable "install whisper" message — it never
// silently fails, and it never falls back to a cloud STT.

const vscode = require("vscode");
const cp = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

class VoiceNavigator {
  /**
   * @param {vscode.ExtensionContext} context
   * @param {import('./backend').ForgeBackend} backend
   * @param {(m: string) => void} log
   */
  constructor(context, backend, log) {
    this.context = context;
    this.backend = backend;
    this.log = log;
    /** @type {vscode.WebviewPanel | undefined} */
    this.panel = undefined;
    /** @type {vscode.Location | undefined} location to return to on Undo */
    this.prevLocation = undefined;
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
    this.status.command = "forge.voiceNavigate";
    this._setState("idle");
    this.status.show();
  }

  _setState(state) {
    const map = {
      idle: "$(mic) Voice",
      listening: "$(mic-filled) Listening…",
      transcribing: "$(loading~spin) Transcribing…",
      resolving: "$(search) Locating…",
    };
    this.status.text = map[state] || map.idle;
    this.status.tooltip = "Voice demo navigation — push-to-talk, jumps the editor to the code you describe (local STT)";
    if (this.panel) this.panel.webview.postMessage({ type: "state", state });
  }

  /** Open (or reveal) the push-to-talk panel. */
  open() {
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Beside, true);
      return;
    }
    this.panel = vscode.window.createWebviewPanel(
      "forgeVoice",
      "Forge Voice",
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      { enableScripts: true, retainContextWhenHidden: true }
    );
    this.panel.webview.html = this._html();
    this.panel.onDidDispose(() => (this.panel = undefined));
    this.panel.webview.onDidReceiveMessage((m) => this._onMessage(m));
  }

  async _onMessage(m) {
    switch (m.type) {
      case "state":
        this._setState(m.state);
        break;
      case "audio":
        await this._handleAudio(m.wavBase64);
        break;
      case "undo":
        await this.undo();
        break;
      default:
        break;
    }
  }

  /** Transcribe -> locate -> reveal. */
  async _handleAudio(wavBase64) {
    let transcript;
    try {
      this._setState("transcribing");
      transcript = await this._transcribe(wavBase64);
    } catch (e) {
      this._setState("idle");
      this._post({ type: "error", message: String((e && e.message) || e) });
      return;
    }
    if (!transcript || !transcript.trim()) {
      this._setState("idle");
      this._post({ type: "heard", transcript: "(nothing heard)", target: null });
      return;
    }
    this._post({ type: "heard", transcript });
    this._setState("resolving");
    try {
      await this.backend.ensureStarted();
      const res = await this.backend.getJson(`/api/voice/locate?q=${encodeURIComponent(transcript)}`);
      if (res.found && res.target) {
        await this._reveal(res.target);
        this._post({ type: "heard", transcript, target: res.target });
      } else {
        this._post({ type: "heard", transcript, target: null, note: res.error || "no matching code" });
      }
    } catch (e) {
      this._post({ type: "error", message: String((e && e.message) || e) });
    }
    this._setState("idle");
  }

  /**
   * Resolve the whisper.cpp binary + model. Prefer explicit settings; otherwise
   * fall back to the copies BUNDLED inside this extension at <ext>/bin, so the
   * desktop app's Voice feature works with zero configuration. Everything stays
   * on-device — audio never leaves the machine.
   * @returns {{ bin: string, model: string }}
   */
  _resolveWhisper() {
    const cfg = vscode.workspace.getConfiguration("forge");
    let bin = cfg.get("whisperPath", "whisper-cli");
    let model = cfg.get("whisperModel", "");
    const binName = process.platform === "win32" ? "whisper-cli.exe" : "whisper-cli";
    const bundledBin = path.join(this.context.extensionPath, "bin", binName);
    const bundledModel = path.join(this.context.extensionPath, "bin", "ggml-base.en.bin");
    if ((!bin || bin === "whisper-cli") && fs.existsSync(bundledBin)) bin = bundledBin;
    if (!model && fs.existsSync(bundledModel)) model = bundledModel;
    return { bin: bin || "whisper-cli", model };
  }

  /** Run a LOCAL whisper.cpp CLI over the captured WAV. */
  async _transcribe(wavBase64) {
    const { bin, model } = this._resolveWhisper();
    if (!model) {
      throw new Error(
        "Local STT not configured. Set forge.whisperModel to a ggml model (e.g. ggml-base.en.bin) and forge.whisperPath to your whisper.cpp binary. Audio stays on-device."
      );
    }
    const wav = path.join(os.tmpdir(), `forge-voice-${Date.now()}.wav`);
    fs.writeFileSync(wav, Buffer.from(wavBase64, "base64"));
    try {
      const text = await new Promise((resolve, reject) => {
        // -nt = no timestamps, -otxt off (read stdout), -m model, -f wav.
        const proc = cp.spawn(bin, ["-m", model, "-f", wav, "-nt"], { stdio: ["ignore", "pipe", "pipe"] });
        let out = "";
        let err = "";
        proc.stdout.on("data", (d) => (out += d.toString()));
        proc.stderr.on("data", (d) => (err += d.toString()));
        proc.on("error", (e) =>
          reject(
            new Error(
              e.code === "ENOENT"
                ? `whisper binary '${bin}' not found. Install whisper.cpp and set forge.whisperPath.`
                : e.message
            )
          )
        );
        proc.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(err || `whisper exited ${code}`))));
      });
      return text.replace(/\[[0-9:.\s\->]+\]/g, "").trim();
    } finally {
      fs.unlink(wav, () => {});
    }
  }

  /** Open the file and reveal the target line; remember where we were for Undo. */
  async _reveal(target) {
    const folders = vscode.workspace.workspaceFolders;
    const root = folders && folders[0] ? folders[0].uri.fsPath : process.cwd();
    const abs = path.isAbsolute(target.file) ? target.file : path.join(root, target.file);
    const active = vscode.window.activeTextEditor;
    if (active) {
      this.prevLocation = new vscode.Location(active.document.uri, active.selection.active);
    }
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(abs));
    const editor = await vscode.window.showTextDocument(doc, { preserveFocus: false });
    const line = Math.max(0, (target.line || 1) - 1);
    const range = new vscode.Range(line, 0, line, 0);
    editor.selection = new vscode.Selection(range.start, range.start);
    editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  }

  /** Undo the last navigation (jump back to where the user was). */
  async undo() {
    if (!this.prevLocation) return;
    const loc = this.prevLocation;
    this.prevLocation = undefined;
    const doc = await vscode.workspace.openTextDocument(loc.uri);
    const editor = await vscode.window.showTextDocument(doc);
    editor.revealRange(new vscode.Range(loc.range.start, loc.range.start), vscode.TextEditorRevealType.InCenter);
  }

  _post(msg) {
    if (this.panel) this.panel.webview.postMessage(msg);
  }

  dispose() {
    this.status.dispose();
    if (this.panel) this.panel.dispose();
  }

  _html() {
    const nonce = nonce32();
    const csp = [
      "default-src 'none'",
      "style-src 'unsafe-inline'",
      `script-src 'nonce-${nonce}'`,
      "media-src 'self' blob:",
      "connect-src 'none'",
    ].join("; ");
    return `<!DOCTYPE html><html lang="en"><head>
<meta charset="UTF-8" />
<meta http-equiv="Content-Security-Policy" content="${csp}" />
<style>
  body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); padding: 14px; }
  .bar { display: flex; align-items: center; gap: 10px; }
  .orb { width: 14px; height: 14px; border-radius: 50%; background: #888; transition: background .2s; }
  .orb.listening { background: #3fb950; animation: pulse 1s infinite; }
  .orb.transcribing, .orb.resolving { background: #d29922; animation: pulse 1s infinite; }
  @keyframes pulse { 0%,100%{opacity:1} 50%{opacity:.4} }
  button { padding: 8px 14px; border: none; border-radius: 6px; cursor: pointer;
    background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
  button:disabled { opacity: .5; cursor: default; }
  .heard { margin-top: 12px; font-size: 13px; }
  .target { margin-top: 6px; opacity: .85; }
  .row { margin-top: 10px; display: flex; gap: 8px; }
  .ghost { background: transparent; color: var(--vscode-textLink-foreground); }
  .hint { margin-top: 14px; font-size: 12px; opacity: .7; line-height: 1.5; }
</style></head>
<body>
  <div class="bar"><span id="orb" class="orb"></span><strong id="label">Push to talk</strong></div>
  <div class="row">
    <button id="talk">🎙 Hold to talk</button>
    <button id="undo" class="ghost" hidden>↩ Undo</button>
  </div>
  <div id="heard" class="heard"></div>
  <div id="target" class="target"></div>
  <div class="hint">Press and hold, say what you want to see ("the login handler", "where we verify the token"), release to navigate. Audio is transcribed on-device.</div>
  <script nonce="${nonce}">
    const vscode = acquireVsCodeApi();
    const $ = (s) => document.querySelector(s);
    let mediaStream, audioCtx, chunks = [], recording = false;

    function setOrb(state){ $("#orb").className = "orb " + (state||""); $("#label").textContent =
      state==="listening"?"Listening…":state==="transcribing"?"Transcribing…":state==="resolving"?"Locating…":"Push to talk"; }

    // Capture raw PCM via Web Audio, encode 16kHz mono WAV (whisper.cpp input).
    async function start(){
      if (recording) return;
      try { mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true }); }
      catch(e){ $("#heard").textContent = "Microphone permission denied."; return; }
      recording = true; chunks = [];
      vscode.postMessage({ type:"state", state:"listening" }); setOrb("listening");
      audioCtx = new (window.AudioContext||window.webkitAudioContext)();
      const src = audioCtx.createMediaStreamSource(mediaStream);
      const node = audioCtx.createScriptProcessor(4096, 1, 1);
      node.onaudioprocess = (e) => { if(recording) chunks.push(new Float32Array(e.inputBuffer.getChannelData(0))); };
      src.connect(node); node.connect(audioCtx.destination);
      window._node = node; window._src = src;
    }
    function stop(){
      if (!recording) return;
      recording = false;
      try { window._node.disconnect(); window._src.disconnect(); } catch(e){}
      const sr = audioCtx.sampleRate;
      mediaStream.getTracks().forEach(t=>t.stop()); audioCtx.close();
      const wav = encodeWav(flatten(chunks), sr, 16000);
      vscode.postMessage({ type:"audio", wavBase64: wav });
    }
    function flatten(bufs){ let len=0; bufs.forEach(b=>len+=b.length); const out=new Float32Array(len); let o=0; bufs.forEach(b=>{out.set(b,o);o+=b.length;}); return out; }
    function downsample(buf, from, to){ if(to===from) return buf; const ratio=from/to; const n=Math.round(buf.length/ratio); const out=new Float32Array(n); for(let i=0;i<n;i++){ out[i]=buf[Math.floor(i*ratio)]; } return out; }
    function encodeWav(samples, sr, target){
      const ds = downsample(samples, sr, target);
      const buf = new ArrayBuffer(44 + ds.length*2); const v = new DataView(buf);
      const ws=(o,s)=>{ for(let i=0;i<s.length;i++) v.setUint8(o+i, s.charCodeAt(i)); };
      ws(0,"RIFF"); v.setUint32(4,36+ds.length*2,true); ws(8,"WAVE"); ws(12,"fmt ");
      v.setUint32(16,16,true); v.setUint16(20,1,true); v.setUint16(22,1,true);
      v.setUint32(24,target,true); v.setUint32(28,target*2,true); v.setUint16(32,2,true); v.setUint16(34,16,true);
      ws(36,"data"); v.setUint32(40,ds.length*2,true);
      let o=44; for(let i=0;i<ds.length;i++,o+=2){ let s=Math.max(-1,Math.min(1,ds[i])); v.setInt16(o, s<0?s*0x8000:s*0x7FFF, true); }
      let bin=""; const bytes=new Uint8Array(buf); for(let i=0;i<bytes.length;i++) bin+=String.fromCharCode(bytes[i]);
      return btoa(bin);
    }

    const talk = $("#talk");
    talk.addEventListener("mousedown", start);
    talk.addEventListener("mouseup", stop);
    talk.addEventListener("mouseleave", () => { if(recording) stop(); });
    $("#undo").addEventListener("click", () => vscode.postMessage({ type:"undo" }));

    window.addEventListener("message", (e) => {
      const m = e.data;
      if (m.type==="state") setOrb(m.state);
      else if (m.type==="heard") {
        setOrb("");
        $("#heard").textContent = "Heard: " + (m.transcript||"");
        if (m.target) { $("#target").textContent = "→ " + m.target.symbol + "  (" + m.target.file + ":" + m.target.line + ")"; $("#undo").hidden=false; }
        else { $("#target").textContent = m.note ? ("• " + m.note) : "• no match"; }
      }
      else if (m.type==="error") { setOrb(""); $("#heard").textContent = "⚠ " + m.message; }
    });
  </script>
</body></html>`;
  }
}

function nonce32() {
  const c = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) s += c.charAt(Math.floor(Math.random() * c.length));
  return s;
}

module.exports = { VoiceNavigator };

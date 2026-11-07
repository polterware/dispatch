import { createSignal, onCleanup, onMount } from "solid-js";
import { render } from "solid-js/web";
import { VoiceBlob } from "../src";
import type { BlobState } from "../src";
import "./style.css";

function Demo() {
  const [state, setState] = createSignal<BlobState>("idle");
  const [level, setLevel] = createSignal(0.3);
  const [light, setLight] = createSignal(false);
  const [compact, setCompact] = createSignal(false);
  const [micOn, setMicOn] = createSignal(false);
  const [fps, setFps] = createSignal("—");
  let levels = { mic: 0, out: 0 };
  let analyser: AnalyserNode | undefined;
  let audioContext: AudioContext | undefined;
  let stream: MediaStream | undefined;
  let source: MediaStreamAudioSourceNode | undefined;
  let meterFrame = 0;
  let meterStart = performance.now();
  let frames = 0;
  let worst = 0;
  let previous = 0;
  let synthFrame = 0;
  const bytes = new Uint8Array(256);

  onMount(() => {
    const frame = (now: number) => {
      if (previous) worst = Math.max(worst, now - previous);
      previous = now; frames++;
      if (analyser && micOn()) {
        analyser.getByteTimeDomainData(bytes);
        let sum = 0;
        for (const b of bytes) { const v = (b - 128) / 128; sum += v * v; }
        levels.mic = Math.sqrt(sum / bytes.length);
      } else if (state() === "listening") levels.mic = level() * (0.6 + Math.sin(now / 210) * 0.4);
      else if (state() === "speaking") levels.out = level() * Math.max(0, Math.sin(now / 85) * Math.sin(now / 305));
      else { levels.mic *= 0.82; levels.out *= 0.82; }
      if (now - meterStart >= 1000) {
        const avg = frames ? (now - meterStart) / frames : 0;
        setFps(`${frames} FPS · ${avg.toFixed(1)} ms avg · ${worst.toFixed(1)} ms worst`);
        frames = 0; worst = 0; meterStart = now;
      }
      meterFrame = requestAnimationFrame(frame);
    };
    meterFrame = requestAnimationFrame(frame);
    onCleanup(() => cancelAnimationFrame(meterFrame));
  });

  const stopMic = () => {
    source?.disconnect(); stream?.getTracks().forEach((track) => track.stop());
    void audioContext?.close(); source = undefined; stream = undefined; analyser = undefined; audioContext = undefined; setMicOn(false);
  };
  const toggleMic = async () => {
    if (micOn()) { stopMic(); return; }
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      audioContext = new AudioContext(); source = audioContext.createMediaStreamSource(stream);
      analyser = audioContext.createAnalyser(); analyser.fftSize = 512; source.connect(analyser);
      setMicOn(true); setState("listening");
    } catch (error) { setState("error"); alert(`Microphone unavailable: ${String(error)}`); }
  };
  onCleanup(() => { stopMic(); cancelAnimationFrame(synthFrame); });

  const choose = (next: BlobState) => { setState(next); if (next !== "listening") stopMic(); };
  return <main class={light() ? "app app--light" : "app"}>
    <header><div><span class="eyebrow">LOCAL VOICE · SHADER STUDY</span><h1>Voice, in motion.</h1><p>A quiet, responsive surface for a voice that lives on your device.</p></div><button class="theme" onClick={() => setLight(!light())}>{light() ? "Dark theme" : "Light theme"}</button></header>
    <section class="showcase">
      <div class="blob-panel"><VoiceBlob state={state} levels={levels} theme={light() ? "light" : "dark"} mode="expanded" label={`Voice interface: ${state()}`} /><div class="readout"><span class={`status-dot status-dot--${state()}`}></span>{state()}</div><small>Expanded · 220 px</small></div>
      <div class="blob-panel compact-panel"><VoiceBlob state={state} levels={levels} theme={light() ? "light" : "dark"} mode="compact" /><div class="readout"><span class={`status-dot status-dot--${state()}`}></span>{state()}</div><small>Compact · 20 px</small></div>
    </section>
    <section class="controls">
      <div class="control-group"><h2>State</h2><div class="button-row">{(["idle", "listening", "processing", "speaking", "error", "disconnected"] as BlobState[]).map((s) => <button class={state() === s ? "selected" : ""} onClick={() => choose(s)}>{s}</button>)}</div></div>
      <div class="control-group"><h2>Signal</h2><label class="slider-label">Input / output level <span>{Math.round(level() * 100)}%</span></label><input aria-label="Signal level" type="range" min="0" max="1" step="0.01" value={level()} onInput={(event) => setLevel(Number(event.currentTarget.value))} /><div class="button-row"><button class={micOn() ? "selected" : ""} onClick={toggleMic}>{micOn() ? "Stop microphone" : "Use microphone"}</button><button onClick={() => {
        if (synthFrame) { cancelAnimationFrame(synthFrame); synthFrame = 0; setState("idle"); return; }
        setState("speaking"); const start = performance.now();
        const speech = (now: number) => { const t = (now - start) / 1000; levels.out = level() * (0.1 + 0.82 * Math.pow(Math.max(0, Math.sin(t * 4.3) * Math.sin(t * 1.27 + 0.9)), 1.8)); synthFrame = requestAnimationFrame(speech); };
        synthFrame = requestAnimationFrame(speech);
      }}>{synthFrame ? "Stop synthetic speech" : "Synthetic speech"}</button></div><p class="hint">The microphone is sampled locally in this demo and is never recorded.</p></div>
      <div class="control-group"><h2>Appearance</h2><div class="button-row"><button class={light() ? "selected" : ""} onClick={() => setLight(!light())}>{light() ? "Light theme on" : "Switch to light"}</button><button class={compact() ? "selected" : ""} onClick={() => setCompact(!compact())}>{compact() ? "Compact preview on" : "Compact preview off"}</button></div><p class="hint">{compact() ? "Compact presentation emphasizes the inline form." : "Both sizes are shown together for comparison."}</p></div>
    </section>
    <footer><span>FRAME METER</span><strong>{fps()}</strong><span class="foot-note">Measured in this tab · per-second average and worst frame interval</span></footer>
  </main>;
}

render(() => <Demo />, document.getElementById("root")!);

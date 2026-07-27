/**
 * noVNC audio passthrough — adds a Sound button to the noVNC toolbar
 * (same style as Fullscreen, Settings, etc.) and streams audio via WebSocket.
 */
(function () {
  "use strict";

  const WS_PORT = 6902;
  const SAMPLE_RATE = 44100;
  const CHANNELS = 2;

  let audioCtx = null;
  let ws = null;
  let audioOn = false;
  let nextStartTime = 0;
  let btn = null;

  // ── Audio playback ────────────────────────────────────────────────
  function startAudio() {
    audioCtx = new (window.AudioContext || window.webkitAudioContext)({
      sampleRate: SAMPLE_RATE,
    });
    nextStartTime = 0;

    const proto = location.protocol === "https:" ? "wss" : "ws";
    ws = new WebSocket(`${proto}://${location.hostname}:${WS_PORT}`);
    ws.binaryType = "arraybuffer";

    ws.onopen = () => setBtnState(true);

    ws.onmessage = (event) => {
      if (!audioCtx || audioCtx.state === "closed") return;
      const int16 = new Int16Array(event.data);
      const numFrames = int16.length / CHANNELS;
      if (numFrames === 0) return;

      const buffer = audioCtx.createBuffer(CHANNELS, numFrames, SAMPLE_RATE);
      for (let ch = 0; ch < CHANNELS; ch++) {
        const data = buffer.getChannelData(ch);
        for (let i = 0; i < numFrames; i++) {
          data[i] = int16[i * CHANNELS + ch] / 32768.0;
        }
      }

      const source = audioCtx.createBufferSource();
      source.buffer = buffer;
      source.connect(audioCtx.destination);

      const now = audioCtx.currentTime;
      if (nextStartTime < now) nextStartTime = now + 0.01;
      source.start(nextStartTime);
      nextStartTime += buffer.duration;
      if (nextStartTime - now > 0.5) nextStartTime = now + 0.01;
    };

    ws.onclose = () => { if (audioOn) setBtnState(false); };
    ws.onerror = () => setBtnState(false);
  }

  function stopAudio() {
    audioOn = false;
    if (ws) { ws.onclose = null; ws.close(); ws = null; }
    if (audioCtx) { audioCtx.close(); audioCtx = null; }
    setBtnState(false);
  }

  function toggleAudio() {
    if (audioOn) stopAudio();
    else { audioOn = true; startAudio(); }
  }

  function setBtnState(on) {
    if (!btn) return;
    audioOn = on;
    if (on) {
      btn.classList.add("noVNC_selected");
      btn.title = "Sound is ON — click to mute";
    } else {
      btn.classList.remove("noVNC_selected");
      btn.title = "Enable Sound";
    }
  }

  // ── Inject button into noVNC toolbar ──────────────────────────────
  function injectButton() {
    if (document.getElementById("wt_audio_button")) return;

    // Find the toolbar scroll area
    const scroll = document.querySelector("#noVNC_control_bar .noVNC_scroll");
    if (!scroll) return;

    // Create the button (same pattern as other noVNC buttons)
    btn = document.createElement("input");
    btn.type = "image";
    btn.id = "wt_audio_button";
    btn.alt = "Sound";
    btn.className = "noVNC_button";
    btn.title = "Enable Sound";
    // Inline SVG data URI (speaker icon)
    btn.src = "data:image/svg+xml," + encodeURIComponent(
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
      '<polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" fill="#fff" stroke="none"/>' +
      '<path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>' +
      '<path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>' +
      '</svg>'
    );

    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      toggleAudio();
    });

    // Insert after the fullscreen button (or at the end of the toolbar)
    const fullscreen = document.getElementById("noVNC_fullscreen_button");
    if (fullscreen && fullscreen.nextSibling) {
      fullscreen.parentNode.insertBefore(btn, fullscreen.nextSibling);
    } else {
      scroll.appendChild(btn);
    }

    // Add a subtle style for the active state
    const style = document.createElement("style");
    style.textContent = `
      #wt_audio_button.noVNC_selected {
        background-color: #E95420 !important;
        border-radius: 5px;
        box-shadow: 0 0 8px rgba(233,84,32,.6);
      }
    `;
    document.head.appendChild(style);
  }

  // ── Init (wait for noVNC UI to be ready) ──────────────────────────
  function init() {
    injectButton();
    if (!document.getElementById("wt_audio_button")) {
      let tries = 0;
      const t = setInterval(() => {
        injectButton();
        if (document.getElementById("wt_audio_button") || ++tries > 40) clearInterval(t);
      }, 250);
    }
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();

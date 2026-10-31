import { api, unwrap } from "@/shared/api/client";
import { createStore } from "@/shared/lib/store";

export type Call = {
  agentId: string;
  phase: "connecting" | "connected" | "failed";
  problem?: "microphone" | "unreachable";
  /** When audio started flowing, for the timer. */
  since?: number;
  isMuted: boolean;
  /** The agent's voice is above the noise floor right now. */
  isSpeaking: boolean;
  /** The newest message when the call began; cards from later messages show on the call. */
  after: number;
};

// One call at a time, kept outside React so it survives moving between conversations.
const call = createStore<Call | undefined>(undefined);
const speaker = new Audio();
let connection: RTCPeerConnection | undefined;
let microphone: MediaStream | undefined;
let stopListening: (() => void) | undefined;
// Bumped whenever a call ends or another starts, so a setup step that finishes late can tell it
// belongs to a call that is gone and undo itself instead of leaving the microphone on.
let attempt = 0;

const ICE_WAIT_MS = 2_000;
const SPEAKING_LEVEL = 0.02;

function updateCall(change: Partial<Call>): void {
  call.update((current) => current && { ...current, ...change });
}

function release(): void {
  attempt += 1;
  stopListening?.();
  stopListening = undefined;
  connection?.close();
  connection = undefined;
  for (const track of microphone?.getTracks() ?? []) track.stop();
  microphone = undefined;
  speaker.pause();
  speaker.srcObject = null;
}

function gathered(peer: RTCPeerConnection): Promise<void> {
  if (peer.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ICE_WAIT_MS);
    peer.addEventListener("icegatheringstatechange", () => {
      if (peer.iceGatheringState !== "complete") return;
      clearTimeout(timer);
      resolve();
    });
  });
}

function listenForSpeech(stream: MediaStream): () => void {
  const context = new AudioContext();
  const analyser = context.createAnalyser();
  analyser.fftSize = 512;
  context.createMediaStreamSource(stream).connect(analyser);
  const samples = new Float32Array(analyser.fftSize);
  let frame = 0;
  const tick = () => {
    analyser.getFloatTimeDomainData(samples);
    let sum = 0;
    for (const sample of samples) sum += sample * sample;
    const isSpeaking = Math.sqrt(sum / samples.length) > SPEAKING_LEVEL;
    if (call.get()?.isSpeaking !== isSpeaking) updateCall({ isSpeaking });
    frame = requestAnimationFrame(tick);
  };
  frame = requestAnimationFrame(tick);
  return () => {
    cancelAnimationFrame(frame);
    void context.close();
  };
}

export async function startCall(agentId: string, after: number): Promise<void> {
  release();
  const mine = attempt;
  const isCurrent = () => attempt === mine;
  call.set({ agentId, phase: "connecting", isMuted: false, isSpeaking: false, after });

  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  } catch {
    if (isCurrent()) updateCall({ phase: "failed", problem: "microphone" });
    return;
  }
  if (!isCurrent()) {
    for (const track of stream.getTracks()) track.stop();
    return;
  }
  microphone = stream;

  const peer = new RTCPeerConnection();
  connection = peer;
  for (const track of stream.getTracks()) peer.addTrack(track, stream);
  peer.addEventListener("track", ({ streams: [remote] }) => {
    if (!remote || !isCurrent()) return;
    speaker.srcObject = remote;
    void speaker.play();
    stopListening?.();
    stopListening = listenForSpeech(remote);
  });
  peer.addEventListener("connectionstatechange", () => {
    if (!isCurrent()) return;
    if (peer.connectionState === "connected") updateCall({ phase: "connected", since: Date.now() });
    if (peer.connectionState === "failed" || peer.connectionState === "closed") {
      if (call.get()?.phase === "connected") endCall();
      else fail();
    }
  });

  try {
    await peer.setLocalDescription(await peer.createOffer());
    await gathered(peer);
    const { sdp } = await unwrap(
      api.POST("/agents/{id}/calls", {
        params: { path: { id: agentId } },
        body: { sdp: peer.localDescription?.sdp ?? "" },
      }),
    );
    if (isCurrent()) await peer.setRemoteDescription({ type: "answer", sdp });
  } catch {
    if (isCurrent()) fail();
  }
}

function fail(): void {
  release();
  updateCall({ phase: "failed", problem: "unreachable" });
}

export function endCall(): void {
  release();
  call.set(undefined);
}

export function toggleMute(): void {
  const isMuted = !call.get()?.isMuted;
  for (const track of microphone?.getAudioTracks() ?? []) track.enabled = !isMuted;
  updateCall({ isMuted });
}

export const canChooseSpeaker = "setSinkId" in HTMLMediaElement.prototype;

export async function chooseSpeaker(deviceId: string): Promise<void> {
  await speaker.setSinkId(deviceId);
}

export const useCall = call.use;

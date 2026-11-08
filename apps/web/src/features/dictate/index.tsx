import { useEffect, useRef, useState } from "react";
import { z } from "zod";
import { upload } from "@/shared/api/client";

const LIMIT_SECONDS = 10 * 60;

export type DictationState = "idle" | "recording" | "transcribing";
export type DictationProblem = "microphone" | "transcription";

const Transcript = z.object({ text: z.string() });

export function useDictation(agentId: string, onText: (text: string) => void) {
  const [state, setState] = useState<DictationState>("idle");
  const [problem, setProblem] = useState<DictationProblem>();
  const [seconds, setSeconds] = useState(0);
  const recorder = useRef<MediaRecorder | undefined>(undefined);
  const keep = useRef(true);

  useEffect(() => {
    if (state !== "recording") return undefined;
    const timer = setInterval(() => setSeconds((value) => value + 1), 1000);
    return () => clearInterval(timer);
  }, [state]);

  useEffect(() => {
    if (seconds >= LIMIT_SECONDS) recorder.current?.stop();
  }, [seconds]);

  useEffect(() => () => recorder.current?.stop(), []);

  async function transcribe(chunks: Blob[], type: string) {
    setState("transcribing");
    const extension = type.includes("mp4") ? "m4a" : "webm";
    const form = new FormData();
    form.append("audio", new Blob(chunks, { type }), `dictation.${extension}`);
    try {
      const { text } = await upload("POST", `/agents/${agentId}/transcriptions`, form, Transcript);
      if (text) onText(text);
    } catch {
      setProblem("transcription");
    } finally {
      setState("idle");
    }
  }

  async function start() {
    setProblem(undefined);
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch {
      setProblem("microphone");
      return;
    }
    const chunks: Blob[] = [];
    const media = new MediaRecorder(stream);
    media.addEventListener("dataavailable", (event) => chunks.push(event.data));
    media.addEventListener("stop", () => {
      for (const track of stream.getTracks()) track.stop();
      recorder.current = undefined;
      if (keep.current && chunks.length > 0) void transcribe(chunks, media.mimeType);
      else setState("idle");
    });
    keep.current = true;
    recorder.current = media;
    setSeconds(0);
    media.start();
    setState("recording");
  }

  function stop() {
    keep.current = true;
    recorder.current?.stop();
  }

  function discard() {
    keep.current = false;
    recorder.current?.stop();
  }

  return { state, problem, seconds, start, stop, discard };
}

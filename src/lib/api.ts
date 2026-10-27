/** The app's commands (src-tauri/src/lib.rs) and their types. */
import { invoke } from "@tauri-apps/api/core";

export type Access = "allowed" | "denied" | "not_asked";

export interface Permissions {
  screen: Access;
  mic: Access;
  /** macOS 12: a new Screen Recording permission applies after a restart. */
  restart_after_screen: boolean;
}

export interface Display {
  id: number;
  name: string;
  width: number;
  height: number;
  main: boolean;
}

export interface Mic {
  id: string;
  name: string;
  default: boolean;
}

export interface Estimate {
  width: number;
  height: number;
  bytes_per_minute: number;
  free_bytes: number | null;
}

export type State = "recording" | "paused" | "finishing" | "done" | "recovered" | "failed";

export interface Status {
  elapsed: number;
  frames: number;
  frames_new: number;
  frames_repeated: number;
  frames_late: number;
  frames_dropped: number;
  audio_samples: number;
  audio_filled: number;
  audio_dropped: number;
  mic_peak: number;
  paused: boolean;
  failed: boolean;
}

export interface TrackCheck {
  codec: string;
  start_s: number;
  duration_s: number;
  packets: number;
}

export interface Check {
  duration_s: number;
  screen: TrackCheck | null;
  mic: TrackCheck | null;
  start_offset_ms: number | null;
  end_offset_ms: number | null;
  recorder: Status | null;
  problems: string[];
  notes: string[];
}

export interface Session {
  id: string;
  created_at: number;
  state: State;
  screen: { display: string; width: number; height: number; fps: number } | null;
  mic: string | null;
  elapsed_s: number;
  pauses: number;
  check: Check | null;
  error: string | null;
  hidden: boolean;
}

export interface Live {
  session: Session;
  status: Status;
  free_bytes: number | null;
}

export interface Options {
  display_id: number;
  display_name: string;
  display_width: number;
  display_height: number;
  quality: number;
  mic_id: string | null;
  mic_name: string | null;
}

export const api = {
  permissions: () => invoke<Permissions>("permissions"),
  requestMic: () => invoke<Access>("request_mic"),
  requestScreen: () => invoke<void>("request_screen"),
  openPrivacySettings: (kind: "screen" | "mic") => invoke<void>("open_privacy_settings", { kind }),
  restartApp: () => invoke<void>("restart_app"),
  devices: () => invoke<{ displays: Display[]; mics: Mic[] }>("devices"),
  estimate: (width: number, height: number, quality: number, mic: boolean) =>
    invoke<Estimate>("estimate", { width, height, quality, mic }),
  monitorStart: (micId: string) => invoke<void>("monitor_start", { micId }),
  monitorLevel: () => invoke<number>("monitor_level"),
  monitorStop: () => invoke<void>("monitor_stop"),
  startRecording: (options: Options) => invoke<Session>("start_recording", { options }),
  pauseRecording: () => invoke<Session>("pause_recording"),
  resumeRecording: () => invoke<Session>("resume_recording"),
  liveStatus: () => invoke<Live | null>("live_status"),
  stopRecording: () => invoke<Session>("stop_recording"),
  listRecordings: () => invoke<Session[]>("list_recordings"),
  removeFromRecent: (id: string) => invoke<void>("remove_from_recent", { id }),
  deleteRecording: (id: string) => invoke<void>("delete_recording", { id }),
  showRecording: (id: string) => invoke<void>("show_recording", { id }),
  pendingRecoveries: () => invoke<Session[]>("pending_recoveries"),
  recoverRecording: (id: string) => invoke<Session>("recover_recording", { id }),
};

/** The app's commands (src-tauri/src/lib.rs) and their types. */
import { invoke } from "@tauri-apps/api/core";

export type Access = "allowed" | "denied" | "not_asked";

export interface Permissions {
  screen: Access;
  mic: Access;
  camera: Access;
  /** macOS 12: a new Screen Recording permission applies after a restart. */
  restart_after_screen: boolean;
  /** Asked in an earlier run and still not allowed (not ticked, or tied to an older copy). */
  screen_asked_before: boolean;
}

export interface Display {
  id: number;
  name: string;
  /** In pixels. */
  width: number;
  height: number;
  main: boolean;
  /** Where it is and its size, in points (top-left of the main display, y down). */
  x: number;
  y: number;
  points_width: number;
  points_height: number;
}

/** A rectangle on a display, in points from its top-left corner. */
export interface Region {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Mic {
  id: string;
  name: string;
  default: boolean;
}

export interface Camera {
  id: string;
  name: string;
  default: boolean;
  /** The largest size it delivers at 30 fps, up to 720p: recorded at this size. */
  width: number;
  height: number;
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
  camera_frames: number;
  camera_frames_new: number;
  camera_frames_repeated: number;
  camera_frames_late: number;
  camera_frames_dropped: number;
  camera_frames_lost: number;
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
  camera: TrackCheck | null;
  mic: TrackCheck | null;
  start_offset_ms: number | null;
  end_offset_ms: number | null;
  camera_start_offset_ms: number | null;
  camera_end_offset_ms: number | null;
  recorder: Status | null;
  problems: string[];
  notes: string[];
}

export interface Session {
  id: string;
  created_at: number;
  state: State;
  screen: { display: string; region: Region | null; width: number; height: number; fps: number } | null;
  mic: string | null;
  camera: { name: string; width: number; height: number } | null;
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
  camera_id: string | null;
  camera_name: string | null;
  camera_width: number;
  camera_height: number;
  display_points_width: number;
  region: Region | null;
}

export const api = {
  permissions: () => invoke<Permissions>("permissions"),
  requestMic: () => invoke<Access>("request_mic"),
  requestCamera: () => invoke<Access>("request_camera"),
  requestScreen: () => invoke<void>("request_screen"),
  resetScreenPermission: () => invoke<void>("reset_screen_permission"),
  openPrivacySettings: (kind: "screen" | "mic" | "camera") => invoke<void>("open_privacy_settings", { kind }),
  restartApp: () => invoke<void>("restart_app"),
  devices: () => invoke<{ displays: Display[]; mics: Mic[]; cameras: Camera[] }>("devices"),
  /** The camera's size, or 0 × 0 when it's off. */
  estimate: (display: Display, region: Region | null, quality: number, cameraWidth: number, cameraHeight: number, mic: boolean) =>
    invoke<Estimate>("estimate", {
      width: display.width,
      height: display.height,
      pointsWidth: display.points_width,
      region,
      quality,
      cameraWidth,
      cameraHeight,
      mic,
    }),
  monitorStart: (micId: string) => invoke<void>("monitor_start", { micId }),
  monitorLevel: () => invoke<number>("monitor_level"),
  monitorStop: () => invoke<void>("monitor_stop"),
  cameraPreviewStart: (cameraId: string, width: number, height: number) =>
    invoke<void>("camera_preview_start", { cameraId, width, height }),
  /** The newest picture after `after`: its number (8 bytes, little-endian) + JPEG; empty if none. */
  cameraPreviewFrame: (after: number) => invoke<ArrayBuffer>("camera_preview_frame", { after }),
  cameraPreviewStop: () => invoke<void>("camera_preview_stop"),
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

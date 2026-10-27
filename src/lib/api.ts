/** The app's commands (src-tauri/src/lib.rs) and their types. */
import { invoke } from "@tauri-apps/api/core";

export type Access = "allowed" | "denied" | "not_asked";

export type Os = "macos" | "windows" | "linux";

export interface Permissions {
  /** Which system's settings the texts name. */
  os: Os;
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

/** A window that can be recorded. */
export interface Window {
  id: number;
  app: string;
  title: string;
  /** In points, and the pixels per point of its display. */
  width: number;
  height: number;
  scale: number;
}

/** The window chosen on Home (sessions.rs `WindowChoice`). */
export interface WindowChoice {
  id: number;
  /** "App – Title". */
  name: string;
  width: number;
  height: number;
  scale: number;
}

export interface Devices {
  displays: Display[];
  mics: Mic[];
  cameras: Camera[];
  /** The computer sound can be recorded; otherwise why not. */
  system_audio: boolean;
  system_audio_note: string | null;
  /** Windows can be recorded; otherwise why not. */
  windows: boolean;
  windows_note: string | null;
  /** A region can be chosen; otherwise why not. */
  regions: boolean;
  regions_note: string | null;
  /** Rec's own windows can be kept out of display and region recordings. */
  own_windows_kept_out: boolean;
  os: Os;
}

export interface Estimate {
  width: number;
  height: number;
  bytes_per_minute: number;
  free_bytes: number | null;
}

export type State = "countdown" | "recording" | "paused" | "finishing" | "done" | "recovered" | "failed";

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
  system_samples?: number;
  system_filled?: number;
  system_dropped?: number;
  window_closed?: boolean;
  window_closed_at?: number;
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
  system?: TrackCheck | null;
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
  screen: {
    display: string;
    region: Region | null;
    /** A window instead of the display: "App – Title". */
    window?: string | null;
    width: number;
    height: number;
    fps: number;
    /** Rec's own windows were kept out of the picture. */
    own_windows_kept_out?: boolean;
  } | null;
  mic: string | null;
  camera: { name: string; width: number; height: number } | null;
  /** The computer's sound was recorded too. */
  system_audio?: boolean;
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
  window: WindowChoice | null;
  system_audio: boolean;
  /** Count 3-2-1 first (the recording is held until the countdown ends). */
  countdown: boolean;
}

/** Finish choices (src-tauri/src/finish.rs). */
export type Layout = "screen" | "bubble" | "camera";
export type BubbleShape = "circle" | "rounded";
export type BubbleSize = "small" | "medium" | "large";
export type Loudness = "social" | "tutorial" | "keep";

export interface Bubble {
  shape: BubbleShape;
  size: BubbleSize;
  /** Fractions (0–1) of the room inside the margin: (1, 1) is bottom right. */
  x: number;
  y: number;
}

export interface FinishSettings {
  start_s: number;
  /** null: to the end. */
  end_s: number | null;
  layout: Layout;
  bubble: Bubble;
  clean_voice: boolean;
  loudness: Loudness;
}

/** Progress of `prepareSound` (event "sound-progress"). */
export interface SoundProgress {
  id: string;
  stage: "clean" | "loudness";
  fraction: number;
}

/** Where the export goes (src-tauri/src/lib.rs `ExportTarget`). */
export interface ExportTarget {
  folder: string;
  name: string;
  /** A file of that name is already there. */
  exists: boolean;
  estimated_bytes: number;
  free_bytes: number | null;
}

/** Progress of an export (event "export-progress"). */
export interface ExportProgress {
  id: string;
  stage: "sound" | "picture" | "check";
  fraction: number;
}

/** A finished, checked export (export.rs `Done`). */
export interface ExportDone {
  path: string;
  facts: { width: number; height: number; video_s: number; audio_s: number; profile: string };
  encoder: string;
  lufs: number | null;
  true_peak_db: number | null;
  /** Quieter than the preset because louder would distort the peaks. */
  held_by_peaks: boolean;
}

/** A recording's tracks (absolute paths; null when not recorded). */
export interface Media {
  screen: string | null;
  camera: string | null;
  mic: string | null;
  system: string | null;
}

/** The test version's tests (src-tauri/src/fieldtest.rs). */
export type FieldTest = "screen" | "region" | "window" | "sync" | "camera" | "sound";

export interface FieldTestInfo {
  app: string;
  os: Os;
  os_version: string;
  arch: string;
  cpus: number;
  displays: Display[];
  mics: { name: string; default: boolean }[];
  cameras: { name: string; width: number; height: number }[];
  computer_sound: boolean;
  windows: boolean;
  encoder: string;
}

export interface FieldTestResult {
  passed: boolean;
  summary: string;
  [numbers: string]: unknown;
}

/** Where the test window is (opaque to the screen; handed back to fieldtestRun). */
export type FieldTestPlace = Record<string, number>;

export const api = {
  permissions: () => invoke<Permissions>("permissions"),
  requestMic: () => invoke<Access>("request_mic"),
  requestCamera: () => invoke<Access>("request_camera"),
  requestScreen: () => invoke<void>("request_screen"),
  resetScreenPermission: () => invoke<void>("reset_screen_permission"),
  openPrivacySettings: (kind: "screen" | "mic" | "camera") => invoke<void>("open_privacy_settings", { kind }),
  restartApp: () => invoke<void>("restart_app"),
  devices: () => invoke<Devices>("devices"),
  /** The windows that can be recorded now. */
  windows: () => invoke<Window[]>("windows"),
  /** The camera's size, or 0 × 0 when it's off. */
  estimate: (
    display: Display,
    region: Region | null,
    quality: number,
    cameraWidth: number,
    cameraHeight: number,
    mic: boolean,
    window: WindowChoice | null,
    systemAudio: boolean,
  ) =>
    invoke<Estimate>("estimate", {
      width: display.width,
      height: display.height,
      pointsWidth: display.points_width,
      region,
      quality,
      cameraWidth,
      cameraHeight,
      mic,
      window,
      systemAudio,
    }),
  monitorStart: (micId: string) => invoke<void>("monitor_start", { micId }),
  monitorLevel: () => invoke<number>("monitor_level"),
  monitorStop: () => invoke<void>("monitor_stop"),
  cameraPreviewStart: (cameraId: string, width: number, height: number) =>
    invoke<void>("camera_preview_start", { cameraId, width, height }),
  /** The newest picture after `after`: its number (8 bytes, little-endian) + JPEG; empty if none. */
  cameraPreviewFrame: (after: number) => invoke<ArrayBuffer>("camera_preview_frame", { after }),
  cameraPreviewStop: () => invoke<void>("camera_preview_stop"),
  /** Opens the overlay to drag a region on `display`; the answer arrives as "region-chosen". */
  regionOpen: (display: Display, region: Region | null) => invoke<void>("region_open", { display, region }),
  /** From the overlay: the chosen region, or null when cancelled. */
  regionDone: (region: Region | null) => invoke<void>("region_done", { region }),
  /** With a countdown: opens it over `display` and holds the recording until it ends. */
  startRecording: (options: Options, display: Display) => invoke<Session>("start_recording", { options, display }),
  /** From the countdown window: begin the recording ("recording-began" goes to the main window). */
  countdownDone: () => invoke<Session>("countdown_done"),
  /** Esc during the countdown: nothing is kept ("recording-cancelled" goes to the main window). */
  countdownCancel: () => invoke<void>("countdown_cancel"),
  countdownPref: () => invoke<boolean>("countdown_pref"),
  /** Moves a floating window by (dx, dy) points (dragged by hand, lib/handDrag.ts). */
  floatingMove: (label: "control" | "bubble", dx: number, dy: number) =>
    invoke<void>("floating_move", { label, dx, dy }),
  bubbleResize: (size: "small" | "medium" | "large") => invoke<void>("bubble_resize", { size }),
  /** Hides the bubble for the rest of the recording (the camera keeps recording). */
  bubbleHide: () => invoke<void>("bubble_hide"),
  setCountdownPref: (on: boolean) => invoke<void>("set_countdown_pref", { on }),
  pauseRecording: () => invoke<Session>("pause_recording"),
  resumeRecording: () => invoke<Session>("resume_recording"),
  liveStatus: () => invoke<Live | null>("live_status"),
  stopRecording: () => invoke<Session>("stop_recording"),
  listRecordings: () => invoke<Session[]>("list_recordings"),
  removeFromRecent: (id: string) => invoke<void>("remove_from_recent", { id }),
  deleteRecording: (id: string) => invoke<void>("delete_recording", { id }),
  showRecording: (id: string) => invoke<void>("show_recording", { id }),
  recording: (id: string) => invoke<Session>("recording", { id }),
  finishSettings: (id: string) => invoke<FinishSettings>("finish_settings", { id }),
  saveFinishSettings: (id: string, settings: FinishSettings) =>
    invoke<void>("save_finish_settings", { id, settings }),
  recordingMedia: (id: string) => invoke<Media>("recording_media", { id }),
  /** One level byte per 10 ms (src-tauri/src/waveform.rs). */
  recordingWaveform: (id: string) => invoke<ArrayBuffer>("recording_waveform", { id }),
  /** The preview's processed sound for these choices (null without sound); rejects "cancelled" when replaced. */
  prepareSound: (id: string, clean: boolean, loudness: Loudness) =>
    invoke<string | null>("prepare_sound", { id, clean, loudness }),
  exportTarget: (id: string) => invoke<ExportTarget>("export_target", { id }),
  setExportFolder: (folder: string) => invoke<void>("set_export_folder", { folder }),
  /** With the saved Finish choices; rejects "cancelled" after Cancel. */
  exportRecording: (id: string, replace: boolean) => invoke<ExportDone>("export_recording", { id, replace }),
  cancelExport: () => invoke<void>("cancel_export"),
  showExport: () => invoke<void>("show_export"),
  pendingRecoveries: () => invoke<Session[]>("pending_recoveries"),
  recoverRecording: (id: string) => invoke<Session>("recover_recording", { id }),
  /** null in releases (no Tests screen). */
  fieldtestInfo: () => invoke<FieldTestInfo | null>("fieldtest_info"),
  fieldtestPattern: (mode: "pattern" | "sound" | "flash") => invoke<FieldTestPlace>("fieldtest_pattern", { mode }),
  fieldtestPatternClose: () => invoke<void>("fieldtest_pattern_close"),
  fieldtestRun: (test: FieldTest, place: FieldTestPlace | null) =>
    invoke<FieldTestResult>("fieldtest_run", { test, place }),
  fieldtestSave: (results: unknown) => invoke<string>("fieldtest_save", { results }),
};

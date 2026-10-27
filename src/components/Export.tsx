import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { ExportDone, ExportProgress, ExportTarget, api } from "../lib/api";
import { exportFraction, exportStageLabel, folderName, summary } from "../lib/exporting";
import { formatBytes } from "../lib/format";
import { Button, Dialog, ProgressBar, SectionLabel, toast } from "../ui";

type Step =
  | { name: "idle" }
  | { name: "conflict"; target: ExportTarget }
  | { name: "running"; progress: ExportProgress | null }
  | { name: "done"; done: ExportDone }
  | { name: "failed"; message: string };

/**
 * Export: where the video goes (remembered folder, Movies by default), a name conflict
 * asked about before anything is written, progress with Cancel, and the checked result.
 * `beforeExport` waits until the Finish choices are saved.
 */
export function useExport(id: string, beforeExport: () => Promise<void>) {
  const [step, setStep] = useState<Step>({ name: "idle" });
  const [target, setTarget] = useState<ExportTarget | null>(null);

  const refresh = (): Promise<ExportTarget | null> =>
    api.exportTarget(id).then(
      (t) => {
        setTarget(t);
        return t;
      },
      (e) => {
        toast(String(e), "error");
        return null;
      },
    );
  useEffect(() => {
    void refresh();
  }, [id]);

  const run = async (replace: boolean) => {
    setStep({ name: "running", progress: null });
    const stop = await listen<ExportProgress>("export-progress", (e) => {
      if (e.payload.id === id) setStep({ name: "running", progress: e.payload });
    });
    try {
      setStep({ name: "done", done: await api.exportRecording(id, replace) });
    } catch (e) {
      setStep(String(e) === "cancelled" ? { name: "idle" } : { name: "failed", message: String(e) });
    } finally {
      stop();
      void refresh();
    }
  };

  const start = async () => {
    await beforeExport();
    const t = await refresh();
    if (!t) return;
    if (t.exists) setStep({ name: "conflict", target: t });
    else void run(false);
  };

  const changeFolder = async () => {
    const folder = await open({ directory: true, defaultPath: target?.folder, title: "Export to this folder" });
    if (typeof folder !== "string") return;
    await api.setExportFolder(folder).catch((e) => toast(String(e), "error"));
    void refresh();
  };

  return { step, setStep, target, start, run, changeFolder, busy: step.name === "running" };
}

export type ExportFlow = ReturnType<typeof useExport>;

/** The inspector's Export section: the folder and the expected size. */
export function ExportSection({ flow }: { flow: ExportFlow }) {
  const t = flow.target;
  return (
    <section className="finish-section">
      <SectionLabel>Export</SectionLabel>
      <div className="finish-folder">
        <FolderOpen size={16} aria-hidden />
        <span className="finish-folder-name" title={t?.folder}>
          {t ? folderName(t.folder) : "…"}
        </span>
        <Button size="sm" disabled={flow.busy} onClick={() => void flow.changeFolder()}>
          Change…
        </Button>
      </div>
      {t && (
        <p className="finish-hint">
          {t.name} · about {formatBytes(t.estimated_bytes)}
          {t.free_bytes !== null ? ` · ${formatBytes(t.free_bytes)} free` : ""}
        </p>
      )}
    </section>
  );
}

/** The dialogs of an export: the name conflict, progress, the result or what went wrong. */
export function ExportDialogs({ flow }: { flow: ExportFlow }) {
  const { step, setStep } = flow;
  const close = () => setStep({ name: "idle" });
  if (step.name === "conflict")
    return (
      <Dialog
        title="A video with this name is already there"
        onClose={close}
        footer={
          <>
            <Button variant="secondary" onClick={close}>
              Cancel
            </Button>
            <Button variant="secondary" onClick={() => void flow.run(false)}>
              Keep both
            </Button>
            <Button variant="primary" onClick={() => void flow.run(true)}>
              Replace
            </Button>
          </>
        }
      >
        <p className="dialog-text">
          “{step.target.name}” is already in {folderName(step.target.folder)}. Replace it, or keep both and save the new
          one with a number added?
        </p>
      </Dialog>
    );
  if (step.name === "running") {
    const fraction = step.progress ? exportFraction(step.progress) : 0;
    return (
      <Dialog
        title="Exporting"
        onClose={() => undefined}
        dismissible={false}
        footer={
          <Button variant="secondary" onClick={() => void api.cancelExport()}>
            Cancel
          </Button>
        }
      >
        <div className="export-progress">
          <span>
            {exportStageLabel(step.progress?.stage ?? "sound")} {Math.round(fraction * 100)}%
          </span>
          <ProgressBar value={fraction * 100} label="Export progress" />
        </div>
      </Dialog>
    );
  }
  if (step.name === "done")
    return (
      <Dialog
        title="Video exported"
        onClose={close}
        footer={
          <>
            <Button variant="secondary" icon={FolderOpen} onClick={() => void api.showExport().catch((e) => toast(String(e), "error"))}>
              Show in folder
            </Button>
            <Button variant="primary" onClick={close}>
              Done
            </Button>
          </>
        }
      >
        <p className="dialog-text">
          <strong>{step.done.path.split(/[\\/]/).pop()}</strong>
        </p>
        <p className="dialog-text">{summary(step.done)}</p>
        {step.done.held_by_peaks && (
          <p className="dialog-text">
            The sound is a little quieter than the loudness you chose: its loudest moments are already at the limit, and
            louder would distort them.
          </p>
        )}
      </Dialog>
    );
  if (step.name === "failed")
    return (
      <Dialog
        title="The export didn't finish"
        onClose={close}
        footer={
          <>
            <Button variant="secondary" onClick={close}>
              Close
            </Button>
            <Button variant="primary" onClick={() => void flow.start()}>
              Export again
            </Button>
          </>
        }
      >
        <p className="dialog-text">{step.message}</p>
      </Dialog>
    );
  return null;
}

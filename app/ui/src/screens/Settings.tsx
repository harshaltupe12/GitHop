import { useEffect, useState } from 'react';
import { Banner } from '../components/Banner';
import { BrandLockup } from '../components/Brand';
import { Button } from '../components/Button';
import { ErrorText } from '../components/ErrorText';
import { ConfirmDialog, Dialog } from '../components/Dialog';
import { Checkbox } from '../components/Field';
import { Ico, Icons } from '../components/icons';
import { ScreenHeader, SettingRow } from '../components/Layout';
import { Pill } from '../components/Pill';
import { Segmented } from '../components/Segmented';
import { Spinner } from '../components/Spinner';
import { useToast } from '../components/Toast';
import { Toggle } from '../components/Toggle';
import { STATE_KEY } from '../lib/appState';
import * as ipc from '../lib/ipc';
import { setQueryData, useQuery } from '../lib/query';
import { KEYS, removeKey } from '../lib/storage';
import type {
  AppState,
  DoctorReport,
  EnforceMode,
  IpcError,
  SettingsPatch,
  WipeReport,
} from '../lib/types';
import { S } from '../strings';
import { ExportDialog, ImportDialog } from './TransferDialogs';

/**
 * Block / Warn (Design §8.5). Strict mode (`block-strict`, set from the CLI) is
 * shown as its own selected option while it's on, so it's never displayed as
 * plain Block or downgraded by a click on the option that already looks selected.
 */
function enforceOptions(current: EnforceMode): { value: EnforceMode; label: string }[] {
  const opts: { value: EnforceMode; label: string }[] = [
    { value: 'block', label: S.settings.block },
    { value: 'warn', label: S.settings.warn },
  ];
  if (current === 'block-strict')
    opts.splice(1, 0, { value: 'block-strict', label: S.settings.strict });
  return opts;
}

export function SettingsScreen({ state }: { state: AppState }) {
  const toast = useToast();
  const [busy, setBusy] = useState<keyof SettingsPatch | null>(null);
  const [dialog, setDialog] = useState<'export' | 'import' | 'doctor' | 'about' | 'wipe' | null>(
    null,
  );
  const info = useQuery('app_info', ipc.appInfo);

  const patch = async (p: SettingsPatch) => {
    const key = Object.keys(p)[0] as keyof SettingsPatch;
    setBusy(key);
    try {
      const settings = await ipc.updateSettings(p);
      setQueryData<AppState | undefined>(STATE_KEY, (s) =>
        s ? { ...s, settings, autostart: p.autostart ?? s.autostart } : s,
      );
      toast({ message: S.settings.saved });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setBusy(null);
    }
  };

  const [savingDiag, setSavingDiag] = useState(false);
  const saveDiagnostics = async () => {
    const path = await ipc
      .pickSavePath(
        S.settings.saveDiagnosticsTitle,
        S.settings.diagnosticsFileName,
        S.settings.diagnosticsFileType,
        ['txt'],
      )
      .catch(() => null);
    if (!path) return;
    setSavingDiag(true);
    try {
      await ipc.saveDiagnostics(path);
      toast({ message: S.settings.diagnosticsSaved });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setSavingDiag(false);
    }
  };

  const enforce = state.settings.enforce;
  const close = () => setDialog(null);

  return (
    <div>
      <ScreenHeader title={S.settings.title} />
      <div className="flex flex-col gap-3">
        <SettingRow
          title={S.settings.enforce}
          titleId="set-enforce"
          helper={
            <>
              {S.settings.enforceHelp}
              {enforce === 'block-strict' ? (
                <span className="mt-1 block">{S.settings.enforceStrict}</span>
              ) : null}
            </>
          }
          helperId="set-enforce-help"
        >
          <Segmented<EnforceMode>
            labelledBy="set-enforce"
            describedBy="set-enforce-help"
            value={enforce}
            busy={busy === 'enforce'}
            onChange={(v) => {
              // Never send a no-op (or a silent strict → block downgrade): only a
              // different option the user picked is saved.
              if (v !== enforce) void patch({ enforce: v });
            }}
            options={enforceOptions(enforce)}
          />
        </SettingRow>

        <SettingRow
          title={S.settings.logging}
          titleId="set-logging"
          helper={S.settings.loggingHelp}
          helperId="set-logging-help"
        >
          <Toggle
            checked={state.settings.logging}
            labelledBy="set-logging"
            describedBy="set-logging-help"
            busy={busy === 'logging'}
            onChange={(v) => void patch({ logging: v })}
          />
        </SettingRow>

        <SettingRow
          title={S.settings.autostart}
          titleId="set-autostart"
          helper={S.settings.autostartHelp}
          helperId="set-autostart-help"
        >
          <Toggle
            checked={state.autostart}
            labelledBy="set-autostart"
            describedBy="set-autostart-help"
            busy={busy === 'autostart'}
            onChange={(v) => void patch({ autostart: v })}
          />
        </SettingRow>

        <SettingRow title={S.settings.move} titleId="set-move" helper={S.settings.moveHelp}>
          <Button icon={Icons.export} onClick={() => setDialog('export')}>
            {S.settings.export}
          </Button>
          <Button icon={Icons.import} onClick={() => setDialog('import')}>
            {S.settings.import}
          </Button>
        </SettingRow>

        <Banner kind="privacy" live={false}>
          {S.settings.privacy}
        </Banner>

        <SettingRow
          title={S.settings.updates}
          titleId="set-updates"
          helper={S.settings.updatesHelp}
        >
          <Button icon={Icons.external} onClick={() => void ipc.openExternal('releases')}>
            {S.settings.checkUpdates}
          </Button>
        </SettingRow>

        <SettingRow
          title={S.settings.diagnostics}
          titleId="set-diag"
          helper={S.settings.diagnosticsHelp}
        >
          <Button onClick={() => setDialog('doctor')}>{S.settings.runDoctor}</Button>
          <Button onClick={() => void saveDiagnostics()} loading={savingDiag}>
            {S.settings.saveDiagnostics}
          </Button>
        </SettingRow>

        <SettingRow
          title={S.settings.about}
          titleId="set-about"
          helper={info.data ? S.settings.aboutHelp(info.data.version) : undefined}
        >
          <Button onClick={() => setDialog('about')}>{S.settings.licences}</Button>
        </SettingRow>

        <SettingRow title={S.settings.remove} titleId="set-wipe" helper={S.settings.removeHelp}>
          <Button variant="danger" icon={Icons.delete} onClick={() => setDialog('wipe')}>
            {S.settings.wipe}
          </Button>
        </SettingRow>
      </div>

      <ExportDialog open={dialog === 'export'} onClose={close} />
      <ImportDialog open={dialog === 'import'} onClose={close} profiles={state.profiles} />
      <DoctorDialog open={dialog === 'doctor'} onClose={close} />
      <AboutDialog
        open={dialog === 'about'}
        onClose={close}
        version={info.data?.version}
        licenses={info.data?.licenses}
        error={info.error?.message}
      />
      <WipeDialog open={dialog === 'wipe'} onClose={close} />
    </div>
  );
}

const DOCTOR_STATUS = {
  ok: { kind: 'ok', text: S.doctor.ok },
  warning: { kind: 'warning', text: S.doctor.warning },
  problem: { kind: 'blocked', text: S.doctor.problem },
} as const;

function DoctorDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const toast = useToast();
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [running, setRunning] = useState(false);

  const run = async () => {
    setRunning(true);
    setError(null);
    try {
      setReport(await ipc.doctor());
    } catch (e) {
      setError(e as IpcError);
    } finally {
      setRunning(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    setReport(null);
    void run();
  }, [open]);

  const copy = async () => {
    if (!report) return;
    const text = report.checks
      .map((c) => `[${c.status}] ${c.title}: ${c.detail}${c.fix ? ` (fix: ${c.fix})` : ''}`)
      .join('\n');
    try {
      await ipc.copyText(text);
      toast({ message: S.doctor.copied });
    } catch (e) {
      toast({ kind: 'error', message: String(e) });
    }
  };

  const allOk = report?.checks.every((c) => c.status === 'ok');

  return (
    <Dialog
      open={open}
      title={S.doctor.title}
      onClose={onClose}
      wide
      footer={
        <>
          <Button icon={Icons.copy} onClick={copy} disabled={!report}>
            {S.doctor.copy}
          </Button>
          <Button variant="primary" onClick={onClose}>
            {S.common.close}
          </Button>
        </>
      }
    >
      {running ? (
        <p role="status" className="flex items-center gap-2 text-ink-muted">
          <Spinner />
          {S.doctor.running}
        </p>
      ) : error ? (
        <Banner kind="blocked" action={{ label: S.common.retry, onClick: () => void run() }}>
          <ErrorText error={error} />
        </Banner>
      ) : report ? (
        <div className="flex flex-col gap-3">
          {allOk ? <Banner kind="success">{S.doctor.allGood}</Banner> : null}
          <ul className="flex flex-col divide-y divide-line-row rounded-lg border border-line">
            {report.checks.map((c) => {
              const s = DOCTOR_STATUS[c.status];
              return (
                <li key={c.id} className="flex flex-col gap-1 px-3 py-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <Pill kind={s.kind}>{s.text}</Pill>
                    <span className="font-medium">{c.title}</span>
                  </div>
                  {c.detail ? <p className="text-small text-ink-muted">{c.detail}</p> : null}
                  {c.fix ? (
                    <p className="text-small">
                      {S.doctor.fix} <span className="font-mono text-mono-small">{c.fix}</span>
                    </p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
    </Dialog>
  );
}

function AboutDialog({
  open,
  onClose,
  version,
  licenses,
  error,
}: {
  open: boolean;
  onClose: () => void;
  version?: string;
  licenses?: string;
  error?: string;
}) {
  return (
    <Dialog
      open={open}
      title={S.about.title}
      onClose={onClose}
      wide
      footer={
        <Button variant="primary" onClick={onClose}>
          {S.common.close}
        </Button>
      }
    >
      <div className="flex flex-col gap-3">
        {/* The dialog title already names the app, so the lockup is decorative. */}
        <BrandLockup height={48} decorative className="mb-1" />
        {version ? <p>{S.about.version(version)}</p> : null}
        {error ? <Banner kind="blocked">{error}</Banner> : null}
        <h3 className="text-body font-medium">{S.about.licences}</h3>
        {/* Scrollable region must be keyboard reachable (axe scrollable-region-focusable). */}
        <pre
          role="region"
          aria-label={S.about.licences}
          tabIndex={0}
          className="max-h-[18rem] overflow-auto whitespace-pre-wrap rounded-md border border-line bg-bg p-3 font-mono text-mono-small text-ink"
        >
          {licenses ?? ''}
        </pre>
      </div>
    </Dialog>
  );
}

/** Core step ids (backup, hooks, identity, unpin, autostart, data, keys) as words. */
function wipeStepLabel(step: string): string {
  return (S.wipeDialog.steps as Record<string, string>)[step] ?? step;
}

function WipeDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [deleteKeys, setDeleteKeys] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<WipeReport | null>(null);

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.wipe({ unpin_repos: true, delete_keys: deleteKeys, keep_data: false });
      // GitHop's data is gone: a reinstall must start with onboarding again.
      if (r.steps.every((x) => x.ok)) {
        removeKey(KEYS.onboardingDone);
        removeKey(KEYS.onboardingStep);
        removeKey(KEYS.scanFolders);
      }
      setReport(r);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  const reset = () => {
    setDeleteKeys(false);
    setError(null);
    setReport(null);
    onClose();
  };

  if (report) {
    return (
      <Dialog
        open={open}
        title={S.wipeDialog.reportTitle}
        onClose={reset}
        footer={
          <Button variant="primary" onClick={reset}>
            {S.common.close}
          </Button>
        }
      >
        <ul className="flex flex-col gap-2">
          {report.steps.map((s, i) => (
            <li key={i} className="flex items-start gap-2">
              <Ico
                icon={s.ok ? Icons.ok : Icons.warning}
                className={s.ok ? 'mt-0.5 text-aqua-strong' : 'mt-0.5 text-blocked-text'}
              />
              <span>
                <span className="sr-only">
                  {s.ok ? S.wipeDialog.stepOk : S.wipeDialog.stepFailed}:{' '}
                </span>
                <span className="font-medium">{wipeStepLabel(s.step)}</span>
                {s.detail ? (
                  <span className="block text-small text-ink-muted">{s.detail}</span>
                ) : null}
              </span>
            </li>
          ))}
        </ul>
      </Dialog>
    );
  }

  return (
    <ConfirmDialog
      open={open}
      title={S.wipeDialog.title}
      word={S.wipeDialog.word}
      typeLabel={S.wipeDialog.typeToConfirm}
      confirmLabel={busy ? S.wipeDialog.wiping : S.wipeDialog.action}
      onConfirm={confirm}
      onClose={reset}
      busy={busy}
      extra={
        <>
          <Checkbox
            label={S.wipeDialog.deleteKeys}
            helper={S.wipeDialog.deleteKeysHelp}
            checked={deleteKeys}
            onChange={setDeleteKeys}
          />
          {error ? (
            <p role="alert" className="text-small text-blocked-text">
              {error}
            </p>
          ) : null}
        </>
      }
    >
      {S.wipeDialog.body}
    </ConfirmDialog>
  );
}

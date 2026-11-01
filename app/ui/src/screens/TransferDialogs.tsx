import { useEffect, useState } from 'react';
import { Avatar } from '../components/Avatar';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { Dialog } from '../components/Dialog';
import { Checkbox, Field, PasswordInput, TextInput } from '../components/Field';
import { Ico, Icons } from '../components/icons';
import { useToast } from '../components/Toast';
import { applyProfiles } from '../lib/appState';
import { cx } from '../lib/cx';
import { normalizePath, passwordStrength, slugify, truncateMiddle } from '../lib/format';
import * as ipc from '../lib/ipc';
import type { ImportAction, ImportPreview, IpcError, ProfileSet } from '../lib/types';
import { S } from '../strings';

const MIN_PW = 12;

function FilePick({
  label,
  value,
  emptyText,
  onPick,
}: {
  label: string;
  value: string;
  emptyText: string;
  onPick: () => void;
}) {
  return (
    <Field label={label}>
      {(p) => (
        <div className="flex items-center gap-2">
          <input
            {...p}
            readOnly
            value={value ? truncateMiddle(value, 44) : emptyText}
            title={value || undefined}
            className={cx(
              'gh-input min-h-8 w-full min-w-0 rounded-md border border-line-input bg-bg px-2.5 font-mono text-mono',
              value ? 'text-ink' : 'text-ink-muted',
            )}
          />
          <Button icon={Icons.folder} onClick={onPick}>
            {S.common.choose}
          </Button>
        </div>
      )}
    </Field>
  );
}

function Strength({ password }: { password: string }) {
  if (!password) return null;
  const s = passwordStrength(password);
  const text = [
    S.exportDialog.strength.weak,
    S.exportDialog.strength.fair,
    S.exportDialog.strength.strong,
  ][s];
  const tone = ['text-blocked-text', 'text-warning-ink', 'text-aqua-text'][s];
  return (
    <p className={cx('flex items-center gap-1 text-small', tone)} aria-live="polite">
      <Ico icon={s === 2 ? Icons.ok : Icons.warning} />
      {S.exportDialog.strength.label}: {text}
    </p>
  );
}

export function ExportDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const toast = useToast();
  const [path, setPath] = useState('');
  const [pw, setPw] = useState('');
  const [pw2, setPw2] = useState('');
  const [includeKeys, setIncludeKeys] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tried, setTried] = useState(false);

  useEffect(() => {
    if (!open) {
      setPath('');
      setPw('');
      setPw2('');
      setIncludeKeys(false);
      setError(null);
      setTried(false);
    }
  }, [open]);

  const short = pw.length > 0 && pw.length < MIN_PW;
  const mismatch = pw2.length > 0 && pw !== pw2;
  const ok = !!path && pw.length >= MIN_PW && pw === pw2;

  const submit = async () => {
    setTried(true);
    if (!ok) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.exportProfiles(path, pw, includeKeys);
      toast({ message: S.exportDialog.done });
      onClose();
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      title={S.exportDialog.title}
      onClose={busy ? () => undefined : onClose}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            {S.common.cancel}
          </Button>
          <Button
            variant="primary"
            icon={Icons.export}
            onClick={submit}
            loading={busy}
            disabled={!ok}
          >
            {S.exportDialog.action}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p>{S.exportDialog.body}</p>
        <FilePick
          label={S.exportDialog.file}
          value={path}
          emptyText={S.exportDialog.noFile}
          onPick={async () => {
            const p = await ipc
              .pickSavePath(
                S.exportDialog.chooseFile,
                S.exportDialog.defaultName,
                S.exportDialog.fileType,
              )
              .catch(() => null);
            if (p) setPath(normalizePath(p));
          }}
        />
        <Field
          label={S.exportDialog.password}
          helper={S.exportDialog.passwordHelp}
          issues={
            short || (tried && pw.length < MIN_PW)
              ? [{ severity: 'error', message: S.exportDialog.tooShort }]
              : []
          }
        >
          {(p) => <PasswordInput {...p} value={pw} onChange={(e) => setPw(e.target.value)} />}
        </Field>
        <Strength password={pw} />
        <Field
          label={S.exportDialog.confirm}
          issues={mismatch ? [{ severity: 'error', message: S.exportDialog.mismatch }] : []}
        >
          {(p) => <PasswordInput {...p} value={pw2} onChange={(e) => setPw2(e.target.value)} />}
        </Field>
        <Checkbox
          label={S.exportDialog.includeKeys}
          helper={S.exportDialog.includeKeysHelp}
          checked={includeKeys}
          onChange={setIncludeKeys}
        />
        {error ? <Banner kind="blocked">{error}</Banner> : null}
      </div>
    </Dialog>
  );
}

type Choice = { action: ImportAction; newId: string };

export function ImportDialog({
  open,
  onClose,
  profiles,
}: {
  open: boolean;
  onClose: () => void;
  profiles: ProfileSet;
}) {
  const toast = useToast();
  const [path, setPath] = useState('');
  const [pw, setPw] = useState('');
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [choices, setChoices] = useState<Record<string, Choice>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setPath('');
      setPw('');
      setPreview(null);
      setChoices({});
      setError(null);
    }
  }, [open]);

  const conflicted = preview ? [...new Set(preview.conflicts.map((c) => c.profile_id))] : [];

  const loadPreview = async () => {
    setBusy(true);
    setError(null);
    try {
      const p = await ipc.importPreview(path, pw);
      setPreview(p);
      const c: Record<string, Choice> = {};
      for (const id of new Set(p.conflicts.map((x) => x.profile_id)))
        c[id] = { action: 'skip', newId: slugify(`${id}-2`) };
      setChoices(c);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      const resolutions = conflicted.map((id) => {
        const c = choices[id] ?? { action: 'skip' as const, newId: '' };
        return { profile_id: id, action: c.action, new_id: c.action === 'rename' ? c.newId : null };
      });
      const set = await ipc.importApply(path, pw, resolutions);
      applyProfiles(set);
      const skipped = resolutions.filter((r) => r.action === 'skip').length;
      toast({ message: S.importDialog.done(preview.profiles.length - skipped) });
      onClose();
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  const existingLabel = (id: string) => profiles.profile.find((p) => p.id === id)?.label ?? id;

  return (
    <Dialog
      open={open}
      title={S.importDialog.title}
      onClose={busy ? () => undefined : onClose}
      wide={!!preview}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            {S.common.cancel}
          </Button>
          {preview ? (
            <Button variant="primary" icon={Icons.import} onClick={apply} loading={busy}>
              {S.importDialog.apply}
            </Button>
          ) : (
            <Button variant="primary" onClick={loadPreview} loading={busy} disabled={!path || !pw}>
              {S.importDialog.preview}
            </Button>
          )}
        </>
      }
    >
      {!preview ? (
        <div className="flex flex-col gap-4">
          <p>{S.importDialog.body}</p>
          <FilePick
            label={S.importDialog.file}
            value={path}
            emptyText={S.importDialog.noFile}
            onPick={async () => {
              const p = await ipc
                .pickFile(S.importDialog.chooseFile, [
                  { name: S.exportDialog.fileType, extensions: ['githop'] },
                ])
                .catch(() => null);
              if (p) setPath(normalizePath(p));
            }}
          />
          <Field label={S.importDialog.password}>
            {(p) => <PasswordInput {...p} value={pw} onChange={(e) => setPw(e.target.value)} />}
          </Field>
          {error ? <Banner kind="blocked">{error}</Banner> : null}
        </div>
      ) : (
        <div className="flex flex-col gap-4">
          <p>
            {S.importDialog.found(preview.profiles.length)}
            {preview.keys.length ? ` · ${S.importDialog.keys(preview.keys.length)}` : ''}
          </p>
          <ul className="flex flex-col gap-2">
            {preview.profiles.map((p) => (
              <li key={p.id} className="flex items-center gap-2">
                <Avatar label={p.label} color={p.color} size={22} />
                <span>{p.label}</span>
                <span className="truncate font-mono text-mono-small text-ink-muted">{p.email}</span>
              </li>
            ))}
          </ul>
          {conflicted.length ? (
            <section aria-labelledby="import-conflicts" className="flex flex-col gap-3">
              <h3 id="import-conflicts" className="text-body font-medium">
                {S.importDialog.conflictsTitle}
              </h3>
              {conflicted.map((id) => {
                const c = choices[id] ?? { action: 'skip', newId: '' };
                const reasons = preview.conflicts.filter((x) => x.profile_id === id);
                return (
                  <div
                    key={id}
                    className="flex flex-col gap-2 rounded-lg border border-warning-border bg-surface p-3"
                  >
                    <ul className="text-small text-warning-ink">
                      {reasons.map((r, i) => (
                        <li key={i} className="flex items-center gap-1">
                          <Ico icon={Icons.warning} />
                          {S.importDialog.conflict(
                            id,
                            S.importDialog.conflictKind[r.kind],
                            existingLabel(r.existing_id),
                          )}
                        </li>
                      ))}
                    </ul>
                    <div className="flex flex-wrap items-end gap-3">
                      <Field label={S.importDialog.action}>
                        {(p) => (
                          <select
                            {...p}
                            value={c.action}
                            onChange={(e) =>
                              setChoices((x) => ({
                                ...x,
                                [id]: { ...c, action: e.target.value as ImportAction },
                              }))
                            }
                            className="gh-input min-h-8 rounded-md border border-line-input bg-surface px-2 text-body"
                          >
                            <option value="skip">{S.importDialog.skip}</option>
                            <option value="replace">{S.importDialog.replace}</option>
                            <option value="rename">{S.importDialog.rename}</option>
                          </select>
                        )}
                      </Field>
                      {c.action === 'rename' ? (
                        <Field label={S.importDialog.newId}>
                          {(p) => (
                            <TextInput
                              {...p}
                              mono
                              value={c.newId}
                              onChange={(e) =>
                                setChoices((x) => ({
                                  ...x,
                                  [id]: { ...c, newId: e.target.value.toLowerCase() },
                                }))
                              }
                            />
                          )}
                        </Field>
                      ) : null}
                    </div>
                  </div>
                );
              })}
            </section>
          ) : null}
          {error ? <Banner kind="blocked">{error}</Banner> : null}
        </div>
      )}
    </Dialog>
  );
}

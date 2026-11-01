import { useEffect, useId, useMemo, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { ChipInput } from '../components/ChipInput';
import { ColorPicker } from '../components/ColorPicker';
import { Checkbox, Field, TextInput } from '../components/Field';
import { FolderField } from '../components/FolderField';
import { Ico, Icons } from '../components/icons';
import { Avatar } from '../components/Avatar';
import { useToast } from '../components/Toast';
import { cx } from '../lib/cx';
import { NOREPLY_TEMPLATE, nextColor, normalizePath, slugify } from '../lib/format';
import * as ipc from '../lib/ipc';
import type { IpcError, Profile, ProfileColor, ProfileSet, ValidationIssue } from '../lib/types';
import { S } from '../strings';

const FIELDS = ['label', 'id', 'name', 'email', 'color', 'owners', 'folders', 'ssh_key'] as const;
type FieldName = (typeof FIELDS)[number];

type KeyMode = 'generate' | 'existing';

export function emptyProfile(set: ProfileSet | undefined, seed?: Partial<Profile>): Profile {
  return {
    id: '',
    label: '',
    name: '',
    email: '',
    color: nextColor(set),
    ssh_key: '',
    owners: [],
    folders: [],
    ...seed,
  };
}

export function ProfileForm({
  profiles,
  original,
  seed,
  onSaved,
  onCancel,
  onDelete,
  embedded = false,
  submitLabel,
  heading,
}: {
  profiles: ProfileSet;
  original?: Profile;
  /** Pre-filled values for a new profile (onboarding). */
  seed?: Partial<Profile>;
  onSaved: (set: ProfileSet, saved: Profile) => void;
  onCancel?: () => void;
  onDelete?: () => void;
  /** Inside onboarding: no sticky bar, no danger zone, no page title. */
  embedded?: boolean;
  submitLabel?: string;
  heading?: string;
}) {
  const toast = useToast();
  const formId = useId();
  const [draft, setDraft] = useState<Profile>(() =>
    original ? { ...original } : emptyProfile(profiles, seed),
  );
  const [idTouched, setIdTouched] = useState(!!original || !!seed?.id);
  const [touched, setTouched] = useState<Set<FieldName>>(new Set());
  const [submitted, setSubmitted] = useState(false);
  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [showNoreplyHelp, setShowNoreplyHelp] = useState(false);

  const [keyMode, setKeyMode] = useState<KeyMode>(original?.ssh_key ? 'existing' : 'generate');
  const [suggested, setSuggested] = useState('');
  const [noPassphrase, setNoPassphrase] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [publicKey, setPublicKey] = useState<string | null>(null);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);

  const refs = useRef<Partial<Record<FieldName, HTMLElement | null>>>({});
  const colourLabelId = `${formId}-colour`;
  const colourHelpId = `${formId}-colour-help`;

  const set = <K extends keyof Profile>(k: K, v: Profile[K]) => {
    setDraft((d) => ({ ...d, [k]: v }));
    setTouched((t) => new Set(t).add(k as FieldName));
  };

  // Profile id follows the label until the user edits it.
  useEffect(() => {
    if (!idTouched) setDraft((d) => ({ ...d, id: slugify(d.label) }));
  }, [draft.label, idTouched]);

  const effectiveKey = keyMode === 'generate' ? draft.ssh_key || suggested : draft.ssh_key;

  // Live validation through the core (no write).
  useEffect(() => {
    let live = true;
    const t = window.setTimeout(() => {
      ipc
        .validateProfile({ ...draft, ssh_key: effectiveKey }, original?.id)
        .then((r) => live && setIssues(r))
        .catch(() => undefined);
    }, 250);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [draft, effectiveKey, original?.id]);

  // Suggested key path for "Generate new key".
  useEffect(() => {
    if (keyMode !== 'generate' || !draft.id) {
      setSuggested('');
      return;
    }
    let live = true;
    ipc
      .suggestKeyPath(draft.id)
      .then((p) => live && setSuggested(normalizePath(p)))
      .catch(() => live && setSuggested(''));
    return () => {
      live = false;
    };
  }, [keyMode, draft.id]);

  // Existing profile: show its public key.
  useEffect(() => {
    if (!original) return;
    let live = true;
    ipc
      .readPublicKey(original.id)
      .then((k) => live && setPublicKey(k.trim()))
      .catch(() => live && setPublicKey(null));
    return () => {
      live = false;
    };
  }, [original]);

  const takenBy = useMemo(() => {
    const m: Partial<Record<ProfileColor, string>> = {};
    for (const p of profiles.profile) if (p.id !== original?.id) m[p.color] = p.label;
    return m;
  }, [profiles, original?.id]);

  const byField = useMemo(() => {
    const m = new Map<string, ValidationIssue[]>();
    for (const i of issues) {
      const f = (FIELDS as readonly string[]).includes(i.field) ? i.field : '_other';
      m.set(f, [...(m.get(f) ?? []), i]);
    }
    return m;
  }, [issues]);

  const visible = (f: FieldName) =>
    submitted || touched.has(f)
      ? (byField.get(f) ?? [])
      : (byField.get(f) ?? []).filter((i) => i.severity !== 'error');
  const otherIssues = submitted ? (byField.get('_other') ?? []) : [];
  const hasErrors = issues.some((i) => i.severity === 'error');

  const onSubmit = async (e?: FormEvent) => {
    e?.preventDefault();
    setSubmitted(true);
    setSaveError(null);
    const toSave: Profile = { ...draft, ssh_key: effectiveKey };
    let current = issues;
    try {
      current = await ipc.validateProfile(toSave, original?.id);
      setIssues(current);
    } catch {
      // Fall through: save_profile validates again.
    }
    const firstErr = current.find((i) => i.severity === 'error');
    if (firstErr) {
      const f = firstErr.field as FieldName;
      refs.current[f]?.focus();
      return;
    }
    setSaving(true);
    try {
      const result = await ipc.saveProfile(toSave, original?.id);
      toast({ message: S.form.saved(toSave.label) });
      onSaved(result, result.profile.find((p) => p.id === toSave.id) ?? toSave);
    } catch (err) {
      const ie = err as IpcError;
      if (ie.code === 'validation' && ie.issues.length) {
        setIssues(ie.issues);
        const f = ie.issues.find((i) => i.severity === 'error')?.field as FieldName | undefined;
        if (f) refs.current[f]?.focus();
      } else {
        setSaveError(ie.message);
      }
    } finally {
      setSaving(false);
    }
  };

  const generate = async () => {
    setKeyError(null);
    setGenerating(true);
    try {
      const pub = await ipc.generateKey(draft.id, noPassphrase);
      setPublicKey(pub.trim());
      if (suggested) set('ssh_key', suggested);
      toast({ message: S.form.keyCreated });
    } catch (err) {
      setKeyError((err as IpcError).message);
    } finally {
      setGenerating(false);
    }
  };

  const loadPublicKeyFor = async (path: string) => {
    set('ssh_key', path);
    if (!original) return;
    try {
      setPublicKey((await ipc.readPublicKey(original.id)).trim());
    } catch {
      setPublicKey(null);
    }
  };

  const copyKey = async () => {
    if (!publicKey) return;
    try {
      await ipc.copyText(publicKey);
      toast({ message: S.form.keyCopied });
    } catch (err) {
      toast({ kind: 'error', message: (err as Error).message || String(err) });
    }
  };

  const test = async () => {
    if (!original) return;
    setTesting(true);
    try {
      const r = await ipc.testConnection(original.id);
      if (r.kind === 'connected') toast({ message: S.profiles.connected(r.user) });
      else if (r.kind === 'denied') toast({ kind: 'warning', message: S.profiles.denied });
      else toast({ kind: 'error', message: S.profiles.failed(r.reason) });
    } catch (err) {
      toast({ kind: 'error', message: S.profiles.failed((err as IpcError).message) });
    } finally {
      setTesting(false);
    }
  };

  const title = heading ?? (original ? S.form.editTitle(original.label) : S.form.addTitle);

  return (
    <form
      onSubmit={onSubmit}
      noValidate
      aria-labelledby={`${formId}-title`}
      className="flex flex-col"
    >
      <div className={cx('flex items-center gap-3', embedded ? 'mb-4' : 'mb-6')}>
        <Avatar label={draft.label || '?'} color={draft.color} size={40} decorative />
        {embedded ? (
          <h2 id={`${formId}-title`} className="text-heading">
            {title}
          </h2>
        ) : (
          <h1 id={`${formId}-title`} className="text-title">
            {title}
          </h1>
        )}
      </div>

      <div className="flex flex-col gap-8">
        {otherIssues.length ? (
          <Banner kind="blocked">
            <ul>
              {otherIssues.map((i, n) => (
                <li key={n}>{i.message}</li>
              ))}
            </ul>
          </Banner>
        ) : null}

        {/* 1. Identity */}
        <FormSection title={S.form.identity}>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label={S.form.label} helper={S.form.labelHelp} issues={visible('label')}>
              {(p) => (
                <TextInput
                  {...p}
                  ref={(el) => {
                    refs.current.label = el;
                  }}
                  value={draft.label}
                  onChange={(e) => set('label', e.target.value)}
                  maxLength={40}
                />
              )}
            </Field>
            <Field label={S.form.id} helper={S.form.idHelp(draft.id)} issues={visible('id')}>
              {(p) => (
                <TextInput
                  {...p}
                  ref={(el) => {
                    refs.current.id = el;
                  }}
                  mono
                  value={draft.id}
                  onChange={(e) => {
                    setIdTouched(true);
                    set('id', e.target.value.toLowerCase());
                  }}
                  maxLength={32}
                />
              )}
            </Field>
          </div>
          <Field label={S.form.name} helper={S.form.nameHelp} issues={visible('name')}>
            {(p) => (
              <TextInput
                {...p}
                ref={(el) => {
                  refs.current.name = el;
                }}
                value={draft.name}
                onChange={(e) => set('name', e.target.value)}
                autoComplete="name"
              />
            )}
          </Field>
          <Field
            label={S.form.email}
            helper={
              <span className="flex flex-col gap-1">
                <span>{S.form.emailHelp}</span>
                {showNoreplyHelp ? <span>{S.form.noreplyHelp}</span> : null}
              </span>
            }
            issues={visible('email')}
          >
            {(p) => (
              <div className="flex items-center gap-2">
                <TextInput
                  {...p}
                  ref={(el) => {
                    refs.current.email = el;
                  }}
                  mono
                  type="email"
                  value={draft.email}
                  onChange={(e) => set('email', e.target.value)}
                />
                <Button
                  onClick={() => {
                    set('email', NOREPLY_TEMPLATE(draft.owners[0] ?? ''));
                    setShowNoreplyHelp(true);
                    refs.current.email?.focus();
                  }}
                >
                  {S.form.useNoreply}
                </Button>
              </div>
            )}
          </Field>
        </FormSection>

        {/* 2. Colour */}
        <FormSection title={S.form.colour} titleId={colourLabelId}>
          <p id={colourHelpId} className="-mt-2 text-small text-ink-muted">
            {S.form.colourHelp}
          </p>
          <div
            ref={(el) => {
              refs.current.color = el?.querySelector<HTMLElement>('[aria-checked="true"]') ?? null;
            }}
          >
            <ColorPicker
              value={draft.color}
              onChange={(c) => set('color', c)}
              takenBy={takenBy}
              labelledBy={colourLabelId}
              describedBy={colourHelpId}
            />
          </div>
          <IssueList issues={visible('color')} />
        </FormSection>

        {/* 3. Owns */}
        <FormSection title={S.form.owns}>
          <Field label={S.form.ownersLabel} helper={S.form.ownersHelp} issues={visible('owners')}>
            {(p) => (
              <div
                ref={(el) => {
                  refs.current.owners = el?.querySelector('input') ?? null;
                }}
              >
                <ChipInput
                  control={p}
                  value={draft.owners}
                  onChange={(v) => set('owners', v)}
                  placeholder={S.form.ownersPlaceholder}
                />
              </div>
            )}
          </Field>
          <Field
            label={S.form.foldersLabel}
            helper={S.form.foldersHelp}
            issues={visible('folders')}
            labelAs="div"
          >
            {() => (
              <div className="flex flex-col gap-2">
                {draft.folders.map((f, i) => (
                  <FolderField
                    key={`${f}-${i}`}
                    value={f}
                    control={{ id: `${formId}-folder-${i}`, 'aria-describedby': undefined }}
                    pickTitle={S.form.chooseFolder}
                    onChange={(v) =>
                      set(
                        'folders',
                        draft.folders.map((x, j) => (j === i ? v : x)),
                      )
                    }
                    onRemove={() =>
                      set(
                        'folders',
                        draft.folders.filter((_, j) => j !== i),
                      )
                    }
                    removeLabel={S.common.remove(f)}
                  />
                ))}
                <div>
                  <Button
                    icon={Icons.add}
                    ref={(el) => {
                      refs.current.folders = el;
                    }}
                    onClick={async () => {
                      const p = await ipc.pickFolder(S.form.chooseFolder);
                      if (p && !draft.folders.includes(normalizePath(p)))
                        set('folders', [...draft.folders, normalizePath(p)]);
                    }}
                  >
                    {S.form.addFolder}
                  </Button>
                </div>
              </div>
            )}
          </Field>
        </FormSection>

        {/* 4. SSH key */}
        <FormSection title={S.form.sshKey}>
          <fieldset className="flex flex-wrap gap-x-6 gap-y-2">
            <legend className="sr-only">{S.form.keyModeLabel}</legend>
            {(['generate', 'existing'] as const).map((m) => (
              <label key={m} className="flex items-center gap-2 text-body">
                <input
                  type="radio"
                  name={`${formId}-keymode`}
                  checked={keyMode === m}
                  onChange={() => setKeyMode(m)}
                />
                {m === 'generate' ? S.form.keyGenerate : S.form.keyExisting}
              </label>
            ))}
          </fieldset>

          {keyMode === 'generate' ? (
            <div className="flex flex-col gap-3 rounded-lg border border-line bg-surface p-4">
              {suggested ? (
                <p className="font-mono text-mono text-ink-muted">{S.form.willCreate(suggested)}</p>
              ) : null}
              <Banner kind="info" live={false}>
                {S.form.passphraseNote}
              </Banner>
              <Checkbox
                label={S.form.noPassphrase}
                checked={noPassphrase}
                onChange={setNoPassphrase}
              />
              <div>
                <Button
                  icon={Icons.key}
                  onClick={generate}
                  loading={generating}
                  disabled={!draft.id}
                  ref={(el) => {
                    if (keyMode === 'generate') refs.current.ssh_key = el;
                  }}
                >
                  {generating ? S.form.generating : S.form.generate}
                </Button>
              </div>
              {keyError ? <Banner kind="blocked">{keyError}</Banner> : null}
              <IssueList issues={visible('ssh_key')} />
            </div>
          ) : (
            <Field label={S.form.keyPath} helper={S.form.keyPathHelp} issues={visible('ssh_key')}>
              {(p) => (
                <div
                  ref={(el) => {
                    if (keyMode === 'existing')
                      refs.current.ssh_key = el?.querySelector('input') ?? null;
                  }}
                >
                  <FolderField
                    mode="file"
                    value={draft.ssh_key}
                    control={p}
                    pickTitle={S.form.chooseKey}
                    emptyText={S.form.noKeyChosen}
                    onChange={loadPublicKeyFor}
                  />
                </div>
              )}
            </Field>
          )}

          {publicKey ? (
            <div className="flex flex-col gap-2">
              <Field label={S.form.publicKey}>
                {(p) => (
                  <textarea
                    {...p}
                    readOnly
                    value={publicKey}
                    rows={3}
                    className="gh-input w-full resize-none break-all rounded-md border border-line-input bg-bg px-2.5 py-1.5 font-mono text-mono-small text-ink"
                  />
                )}
              </Field>
              <div className="flex flex-wrap gap-2">
                <Button icon={Icons.copy} onClick={copyKey}>
                  {S.form.copyKey}
                </Button>
                <Button icon={Icons.external} onClick={() => void ipc.openExternal('ssh-keys')}>
                  {S.form.openGithub}
                </Button>
                <Button icon={Icons.test} onClick={test} loading={testing} disabled={!original}>
                  {S.form.testConnection}
                </Button>
              </div>
              {!original ? <p className="text-small text-ink-muted">{S.form.saveFirst}</p> : null}
            </div>
          ) : original && keyMode === 'existing' && draft.ssh_key ? (
            <p className="text-small text-ink-muted">{S.form.noPublicKey}</p>
          ) : null}
        </FormSection>

        {/* 5. Danger zone */}
        {original && onDelete && !embedded ? (
          <FormSection title={S.form.dangerZone}>
            <div className="flex flex-wrap items-center gap-4 rounded-lg border border-blocked-border bg-surface px-4 py-3">
              <p className="min-w-[12rem] flex-1 text-small text-ink-muted">{S.form.deleteHelp}</p>
              <Button variant="danger" icon={Icons.delete} onClick={onDelete}>
                {S.form.deleteProfile}
              </Button>
            </div>
          </FormSection>
        ) : null}
      </div>

      <div
        className={cx(
          'flex flex-wrap items-center justify-end gap-2 py-3',
          embedded ? 'mt-6' : 'sticky bottom-0 z-10 -mx-6 mt-8 border-t border-line bg-bg px-6',
        )}
      >
        {submitted && hasErrors ? (
          <p className="mr-auto flex items-center gap-1 text-small text-blocked-text" role="alert">
            <Ico icon={Icons.warning} />
            {S.form.fixErrors}
          </p>
        ) : null}
        {saveError ? (
          <p className="mr-auto flex items-center gap-1 text-small text-blocked-text" role="alert">
            <Ico icon={Icons.warning} />
            {saveError}
          </p>
        ) : null}
        {onCancel ? <Button onClick={onCancel}>{S.common.cancel}</Button> : null}
        <Button variant="primary" type="submit" loading={saving}>
          {saving ? S.form.saving : (submitLabel ?? S.form.save)}
        </Button>
      </div>
    </form>
  );
}

function FormSection({
  title,
  titleId,
  children,
}: {
  title: string;
  titleId?: string;
  children: ReactNode;
}) {
  const autoId = useId();
  const id = titleId ?? autoId;
  return (
    <section aria-labelledby={id} className="flex flex-col gap-4">
      <h2 id={id} className="text-heading">
        {title}
      </h2>
      {children}
    </section>
  );
}

function IssueList({ issues }: { issues: ValidationIssue[] }) {
  if (!issues.length) return null;
  return (
    <ul className="flex flex-col gap-0.5">
      {issues.map((i, n) => (
        <li
          key={n}
          className={cx(
            'flex items-start gap-1 text-small',
            i.severity === 'error'
              ? 'text-blocked-text'
              : i.severity === 'warning'
                ? 'text-warning-ink'
                : 'text-ink-muted',
          )}
        >
          <Ico icon={i.severity === 'info' ? Icons.info : Icons.warning} className="mt-px" />
          {i.message}
        </li>
      ))}
    </ul>
  );
}
